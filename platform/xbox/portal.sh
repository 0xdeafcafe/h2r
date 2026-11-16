# Xbox Device Portal helpers for deploy.sh (docs/runtime/xbox.md). Sourced;
# platform/xbox/tests/portal_test.sh checks them with a stand-in curl.
#
# The portal is HTTPS on port 11443 with the console's self-signed
# certificate (so curl -k), Basic authentication, and a CSRF check on
# writes: the first response sets a `CSRF-Token` cookie, and every POST,
# PUT or DELETE must send its value back as the `X-CSRF-Token` header
# (with the cookie).

# The portal's base URL from H2_XBOX: a host, host:port or URL. A bare
# host gets https:// and :11443.
portal_base() {
  b=${1%/}
  case "$b" in
    http://*|https://*) ;;
    *) b="https://$b" ;;
  esac
  rest=${b#*://}
  case "$rest" in
    *:*) ;;
    *) b="$b:11443" ;;
  esac
  printf '%s\n' "$b"
}

# The host part of a base URL (for the netrc file's machine line).
portal_host() {
  h=${1#*://}
  h=${h%%/*}
  printf '%s\n' "${h%:*}"
}

# Base64 of a string, one line: how the task manager takes app ids.
portal_b64() {
  printf '%s' "$1" | base64 | tr -d '\n'
}

# The CSRF-Token cookie's value from a curl cookie jar, or nothing.
portal_csrf() {
  [ -f "$1" ] || return 0
  awk -F '\t' '$6 == "CSRF-Token" { v = $7 } END { if (v != "") print v }' "$1"
}

# Writes a netrc file (mode 600) for host $2, user $3, password on stdin, to $1.
# The password never appears on a command line.
portal_netrc() {
  (
    umask 077
    IFS= read -r pw
    printf 'machine %s login %s password %s\n' "$2" "$3" "$pw" >"$1"
  )
}

# portal_curl METHOD PATH [curl args...]: a request with the credentials
# (PORTAL_NETRC), the cookie jar (PORTAL_JAR) and, for writes, the CSRF
# header. Prints the body; the HTTP status goes to PORTAL_STATUS_FILE.
portal_curl() {
  method=$1
  path=$2
  shift 2
  if [ "$method" != GET ]; then
    if [ -z "$(portal_csrf "$PORTAL_JAR")" ]; then
      # Any read sets the cookie.
      curl -sk --netrc-file "$PORTAL_NETRC" -b "$PORTAL_JAR" -c "$PORTAL_JAR" -o /dev/null \
        "$PORTAL_BASE/api/os/info" || true
    fi
    token=$(portal_csrf "$PORTAL_JAR")
    set -- -H "X-CSRF-Token: $token" "$@"
  fi
  curl -sk --netrc-file "$PORTAL_NETRC" -b "$PORTAL_JAR" -c "$PORTAL_JAR" \
    -X "$method" -w '%{http_code}' -o "$PORTAL_BODY" "$@" "$PORTAL_BASE$path" >"$PORTAL_STATUS_FILE"
  cat "$PORTAL_BODY"
}

portal_status() {
  cat "$PORTAL_STATUS_FILE"
}

# portal_install PACKAGE [CERTIFICATE]: uploads the package, and with it
# the signing certificate when given (the console must trust a
# self-signed package's certificate; the portal installs a .cer sent in
# the same multipart request).
portal_install() {
  pkg=$1
  cer=${2:-}
  path="/api/app/packagemanager/package?package=$(basename "$pkg")"
  if [ -n "$cer" ]; then
    portal_curl POST "$path" -F "$(basename "$pkg")=@$pkg" -F "$(basename "$cer")=@$cer"
  else
    portal_curl POST "$path" -F "$(basename "$pkg")=@$pkg"
  fi
}

# The installed package whose Name is $2, from the package list JSON in
# file $1: prints "<PackageFullName> <PackageRelativeId>".
portal_find_package() {
  jq -r --arg n "$2" '.InstalledPackages[] | select(.Name == $n or (.PackageFamilyName | startswith($n + "_"))) | "\(.PackageFullName) \(.PackageRelativeId)"' "$1" | head -n 1
}
