#!/bin/sh
# Checks platform/xbox/portal.sh and deploy.sh against a stand-in curl and
# keychain (run by crates/uwp-host/tests/deploy.rs). No network.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/h2xbox-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
eq() { [ "$1" = "$2" ] || fail "$3: got [$1], want [$2]"; }

. "$here/portal.sh"

# Addresses.
eq "$(portal_base 192.168.1.50)" "https://192.168.1.50:11443" "bare host"
eq "$(portal_base https://xbox.lan:11443/)" "https://xbox.lan:11443" "full url"
eq "$(portal_base xbox.lan:8443)" "https://xbox.lan:8443" "host and port"
eq "$(portal_host https://192.168.1.50:11443)" "192.168.1.50" "host part"
eq "$(portal_b64 'h2recomp.Halo2_abc!App')" "aDJyZWNvbXAuSGFsbzJfYWJjIUFwcA==" "base64 aumid"

# Cookie jar parsing (curl's Netscape format, tab-separated).
printf '# Netscape HTTP Cookie File\n192.168.1.50\tFALSE\t/\tTRUE\t0\tWMF\tx\n#HttpOnly_192.168.1.50\tFALSE\t/\tTRUE\t0\tCSRF-Token\ttok123\n' >"$work/jar"
eq "$(portal_csrf "$work/jar")" "tok123" "csrf from jar"
eq "$(portal_csrf "$work/none")" "" "no jar"

# The netrc file: mode 600, password from stdin only.
printf 'pa ss\n' | portal_netrc "$work/netrc" 192.168.1.50 afr
eq "$(cat "$work/netrc")" "machine 192.168.1.50 login afr password pa ss" "netrc"
eq "$(stat -f %Lp "$work/netrc" 2>/dev/null || stat -c %a "$work/netrc")" "600" "netrc mode"

# Package lookup in the portal's list.
cat >"$work/pkgs.json" <<'JSON'
{"InstalledPackages":[
 {"Name":"Other","PackageFullName":"Other_1_x64__x","PackageRelativeId":"Other_x!App"},
 {"Name":"Halo 2 (h2recomp)","PackageFullName":"h2recomp.Halo2_1.0.0.0_x64__q8bw2r7hka0ma","PackageRelativeId":"h2recomp.Halo2_q8bw2r7hka0ma!App"}
]}
JSON
eq "$(portal_find_package "$work/pkgs.json" h2recomp.Halo2)" \
  "h2recomp.Halo2_1.0.0.0_x64__q8bw2r7hka0ma h2recomp.Halo2_q8bw2r7hka0ma!App" "find package"
eq "$(portal_find_package "$work/pkgs.json" Missing)" "" "missing package"
# Older portals give PackageFamilyName too.
printf '{"InstalledPackages":[{"Name":"X","PackageFamilyName":"h2recomp.Halo2_f","PackageFullName":"F","PackageRelativeId":"R"}]}' >"$work/fam.json"
eq "$(portal_find_package "$work/fam.json" h2recomp.Halo2)" "F R" "find by family name"

# deploy.sh end to end against stand-ins.
mkdir -p "$work/bin"
cat >"$work/bin/security" <<'SH'
#!/bin/sh
[ "$*" = "find-generic-password -s h2-xbox-device-portal -a afr -w" ] || { echo "security: $*" >&2; exit 1; }
echo 'secret pw'
SH
cat >"$work/bin/curl" <<'SH'
#!/bin/sh
# Records each call; answers like the portal.
log=$STUB_DIR/calls
jar= out= wfmt= netrc= method=GET url= csrf= forms= cl= data=
while [ $# -gt 0 ]; do
  case "$1" in
    -c) jar=$2; shift ;;
    -b) shift ;;
    -o) out=$2; shift ;;
    -w) wfmt=$2; shift ;;
    -X) method=$2; shift ;;
    --netrc-file) netrc=$2; shift ;;
    -H) case "$2" in X-CSRF-Token:*) csrf=${2#X-CSRF-Token: } ;; Content-Length:*) cl=${2#Content-Length: } ;; esac; shift ;;
    -d) data=1; shift ;;
    -F) forms="$forms ${2%%=*}"
        case "$2" in *=@*) up=${2#*=@}; cp "${up%%;*}" "$STUB_DIR/upload"; echo "${up#*;}" >"$STUB_DIR/upload.name" ;; esac
        shift ;;
    -sk) ;;
    *) url=$1 ;;
  esac
  shift
done
[ "$(cat "$netrc")" = "machine 10.0.0.9 login afr password secret pw" ] || { echo "bad netrc" >&2; exit 9; }
printf '%s %s csrf=%s forms=%s\n' "$method" "${url#https://10.0.0.9:11443}" "$csrf" "$forms" >>"$log"
printf '10.0.0.9\tFALSE\t/\tTRUE\t0\tCSRF-Token\ttok123\n' >"$jar"
status=200 body='{}'
case "$method ${url#https://10.0.0.9:11443}" in
  "GET /api/os/info") ;;
  "POST /api/app/packagemanager/package"*) [ "$csrf" = tok123 ] || status=403; status=${status%200}; status=${status:-202} ;;
  "GET /api/app/packagemanager/state")
    n=$(cat "$STUB_DIR/n" 2>/dev/null || echo 0); echo $((n + 1)) >"$STUB_DIR/n"
    if [ "$n" -lt 2 ]; then status=204 body=; else body='{"Success":true}'; fi ;;
  "GET /api/app/packagemanager/packages") body=$(cat "$STUB_DIR/pkgs.json") ;;
  # The real portal answers 411 to a POST with no body length.
  "POST /api/taskmanager/app"*) if [ "$csrf" != tok123 ]; then status=403; elif [ "$cl" != 0 ] || [ -z "$data" ]; then status=411; fi ;;
  "DELETE /api/taskmanager/app"*) [ "$csrf" = tok123 ] || status=403 ;;
  "GET /api/filesystem/apps/file"*) body=$(cat "$STUB_DIR/file" 2>/dev/null || echo missing) ;;
  "POST /api/filesystem/apps/file"* | "DELETE /api/filesystem/apps/file"*) [ "$csrf" = tok123 ] || status=403 ;;
  *) status=404 ;;
esac
[ -n "$out" ] && printf '%s' "$body" >"$out"
[ -n "$wfmt" ] && printf '%s' "$status"
exit 0
SH
chmod +x "$work/bin/curl" "$work/bin/security"
cp "$work/pkgs.json" "$work/pkgs.json.bak"
mkdir -p "$work/stub"; cp "$work/pkgs.json" "$work/stub/pkgs.json"
printf 'msix' >"$work/h2recomp.msix"
printf 'cer' >"$work/h2recomp.cer"
PATH="$work/bin:$PATH" STUB_DIR="$work/stub" H2_XBOX=10.0.0.9 TMPDIR="$work" \
  sh "$here/deploy.sh" "$work/h2recomp.msix" >"$work/out" 2>&1 || { cat "$work/out"; fail "deploy.sh failed"; }
eq "$(cat "$work/stub/calls")" "GET /api/os/info csrf= forms=
POST /api/app/packagemanager/package?package=h2recomp.msix csrf=tok123 forms= h2recomp.msix h2recomp.cer
GET /api/app/packagemanager/state csrf= forms=
GET /api/app/packagemanager/state csrf= forms=
GET /api/app/packagemanager/state csrf= forms=
GET /api/app/packagemanager/packages csrf= forms=
POST /api/taskmanager/app?appid=aDJyZWNvbXAuSGFsbzJfcThidzJyN2hrYTBtYSFBcHA=&package=aDJyZWNvbXAuSGFsbzJfMS4wLjAuMF94NjRfX3E4Yncycjdoa2EwbWE= csrf=tok123 forms=" "deploy requests"
grep -q "secret" "$work/out" && fail "the password reached the output"

# --launch-only skips the upload and the install poll.
: >"$work/stub/calls"
PATH="$work/bin:$PATH" STUB_DIR="$work/stub" H2_XBOX=10.0.0.9 TMPDIR="$work" \
  sh "$here/deploy.sh" --launch-only "$work/h2recomp.msix" >"$work/out" 2>&1 || { cat "$work/out"; fail "deploy.sh --launch-only failed"; }
eq "$(cut -d' ' -f1-2 "$work/stub/calls")" "GET /api/os/info
GET /api/app/packagemanager/packages
POST /api/taskmanager/app?appid=aDJyZWNvbXAuSGFsbzJfcThidzJyN2hrYTBtYSFBcHA=&package=aDJyZWNvbXAuSGFsbzJfMS4wLjAuMF94NjRfX3E4Yncycjdoa2EwbWE=" "launch-only requests"
# The netrc file is gone with deploy.sh's work directory.
[ -z "$(find "$work" -name netrc -path '*h2xbox.*')" ] || fail "netrc left behind"

# Without an address it stops before touching anything.
if H2_XBOX= sh "$here/deploy.sh" "$work/h2recomp.msix" >/dev/null 2>&1; then fail "ran without H2_XBOX"; fi

# xbox.sh: LocalState files and app control through the same portal.
full=h2recomp.Halo2_1.0.0.0_x64__q8bw2r7hka0ma
ls_query="knownfolderid=LocalAppData&packagefullname=$full&path=%5CLocalState"
b64full=aDJyZWNvbXAuSGFsbzJfMS4wLjAuMF94NjRfX3E4Yncycjdoa2EwbWE=
launch="POST /api/taskmanager/app?appid=aDJyZWNvbXAuSGFsbzJfcThidzJyN2hrYTBtYSFBcHA=&package=$b64full"
xbox() {
  : >"$work/stub/calls"
  PATH="$work/bin:$PATH" STUB_DIR="$work/stub" H2_XBOX=10.0.0.9 TMPDIR="$work" \
    sh "$here/xbox.sh" "$@" >"$work/out" 2>&1 || { cat "$work/out"; fail "xbox.sh $* failed"; }
}
xbox level 03a_oldmombasa heroic
eq "$(cut -d' ' -f1-3 "$work/stub/calls")" "GET /api/app/packagemanager/packages csrf=
POST /api/filesystem/apps/file?$ls_query csrf=tok123
DELETE /api/taskmanager/app?package=$b64full&forcestop=yes csrf=tok123
$launch csrf=tok123" "level requests"
eq "$(cat "$work/stub/upload")" "H2_LEVEL=03a_oldmombasa
H2_DIFFICULTY=heroic" "level writes h2env"
eq "$(cat "$work/stub/upload.name")" "filename=h2env" "the upload's name"
xbox level menu
eq "$(sed -n 2p "$work/stub/calls" | cut -d' ' -f1-2)" "DELETE /api/filesystem/apps/file?$ls_query&filename=h2env" "menu removes h2env"
printf 'h2uwp: starting\n' >"$work/stub/file"
xbox log
eq "$(cat "$work/out")" "h2uwp: starting" "log prints the file"
eq "$(sed -n 2p "$work/stub/calls" | cut -d' ' -f1-2)" "GET /api/filesystem/apps/file?$ls_query&filename=h2uwp.log" "log fetches h2uwp.log"
if PATH="$work/bin:$PATH" STUB_DIR="$work/stub" H2_XBOX=10.0.0.9 TMPDIR="$work" sh "$here/xbox.sh" env PATH=/x >/dev/null 2>&1; then
  fail "env took a variable that isn't H2_"
fi
grep -q "secret" "$work/out" && fail "the password reached xbox.sh's output"

# profile: the agent's sampler, the dump fetched, then symbolised with
# build/symbols (stand-ins for h2 agent and the symboliser).
printf '#!/bin/sh\necho "agent $*" >>"$STUB_DIR/agent"\n' >"$work/bin/fakeagent"
printf '#!/bin/sh\necho "symbolize $*"\n' >"$work/bin/fakesym"
chmod +x "$work/bin/fakeagent" "$work/bin/fakesym"
mkdir -p "$work/syms"
: >"$work/syms/h2game.dll"; : >"$work/syms/h2uwp.exe"; : >"$work/syms/h2uwp.map"
printf 'h2prof 1\n' >"$work/stub/file"
H2_XBOX_AGENT_CMD="$work/bin/fakeagent" H2_XBOX_SYMBOLIZE_CMD="$work/bin/fakesym" \
  H2_XBOX_SYMBOLS="$work/syms" H2_XBOX_PROFILE_DIR="$work" xbox profile 0 500
eq "$(cat "$work/stub/agent")" "agent --xbox 10.0.0.9 sample-start 500
agent --xbox 10.0.0.9 sample-stop" "profile drives the sampler"
eq "$(sed -n 2p "$work/stub/calls" | cut -d' ' -f1-2)" "GET /api/filesystem/apps/file?$ls_query&filename=profile.h2prof" "profile fetches the dump"
dump=$(ls "$work"/xbox-*.h2prof)
eq "$(cat "$dump")" "h2prof 1" "the dump is kept"
grep -q "^symbolize $dump --image $work/syms/h2game.dll --image $work/syms/h2uwp.exe --map $work/syms/h2uwp.map" "$work/out" ||
  { cat "$work/out"; fail "profile symbolises with build/symbols"; }
echo "portal_test: ok"
