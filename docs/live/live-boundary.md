# Halo 2 retail Xbox Live API boundary

Static investigation, 2026-11-15. Design: [live.md](live.md). Target: original Xbox retail `default.xbe`, XDK/XONLINES 5849. Addresses are guest virtual addresses. This report contains derived names, addresses, ABI facts and control-flow observations; no instruction-byte sequences, retail lookup tables, account data or executable extracts.

## Result and scope

The supplied scan has **128 verified targets / 182 direct call sites**, after excluding **three false targets / three false sites**. That is 92 targets / 104 sites in XONLINE and 36 targets / 78 sites in XNET. The sign-in APIs, XNet startup/cleanup, socket APIs, link status, secure-address helpers and QoS entry points are confirmed.

Two additions make the inventory below **130 targets**:

- `0x003ab055`, proposed `XOnlineMessageDelete`, is reached by a verified tail jump at `0x0008e93b`; the original call-only scan missed it.
- `0x0036c47d`, confirmed `XNetGetEthernetLinkStatus`, is in `.text`, outside the two scanned library sections. Game callers are `0x0008d6b0`, `0x0008d7eb`, `0x0008d8bd`; XOnline also calls it internally at `0x003a5007`.

There is another verified tail jump at `0x0008d9de` to the already-listed `XNetCleanup` entry `0x003cd67b`. Thus the expanded inventory has **185 direct calls plus two tail jumps**. Counts in the table are direct calls only; the evidence notes the tail jumps. Counts retain the original scan's source-section scope (`.text`, XPP, D3D, DSOUND, BINK). In particular, two callers of the 16-bit endian helper are in DSOUND; this is a section-based dependency inventory, not a claim that every source section contains only game code.

**Confidence:** `confirmed` means the entry is statically identified from ABI and discriminating caller/callee behavior; `likely` means a consistent proposed name with matching ABI and contextual/public-reference support, without the same depth of independent semantic checking. There are **62 confirmed and 68 likely names** in the expanded inventory. No remaining verified target lacks a proposed name, but the likely names are not implementation-ready contracts.

**ABI column:** `C args / stack words` distinguishes parameters from four-byte stdcall argument slots. The latter equals callee cleanup bytes divided by four, traced through the public thunk to its body. In this image a by-value `XUID` takes three words, `XNKID` two, and `ULONGLONG`/`FILETIME` two. For example, `XOnlineNotificationSetState` has five C parameters but six stack words. Every named entry's observed cleanup matches its proposed public prototype. ECX used for the library singleton is not a public argument.

## Verified target inventory

| Address | Section | Direct sites | Proposed XDK name | Confidence | C args / stack words | Evidence |
|---|---|---:|---|---|---:|---|
| `0x0036c47d` | .text | 3 | `XNetGetEthernetLinkStatus` | confirmed | 0 / 0 | In .text outside the scanned sections; PHY mask returned only if link-active bit is set. |
| `0x003a0620` | XONLINE | 1 | `XOnlineTaskContinue` | confirmed | 1 / 1 | Dispatches task work callback; pumped by 0x0006c670 at 0x0006c6ab. |
| `0x003a062b` | XONLINE | 1 | `XOnlineTaskClose` | confirmed | 1 / 1 | Dispatches task cleanup callback; disposer 0x0006b640 calls it at 0x0006b6cb. |
| `0x003a0636` | XONLINE | 1 | `XOnlineGetUsers` | confirmed | 2 / 2 | Enumerates HDD/MU accounts into 16-entry user buffer; 0x0006c800 supplies users/count. |
| `0x003a0641` | XONLINE | 1 | `XOnlineLogon` | confirmed | 5 / 5 | 0x0006c8b0 passes four-user array, 13 services, event 0, task output; body checks PHY bit 0. |
| `0x003a0650` | XONLINE | 1 | `XOnlineLogonTaskGetResults` | confirmed | 1 / 1 | Returns cached logon task result without pumping; 0x0006cd50 classifies exact logon HRESULTs. |
| `0x003a065b` | XONLINE | 2 | `XOnlineChangeLogonUsers` | confirmed | 3 / 3 | Existing-session branch of 0x0006c8b0 passes replacement four-user array, event 0, task output. |
| `0x003a0666` | XONLINE | 1 | `XOnlineChangeLogonUsersTaskGetResults` | confirmed | 2 / 2 | 0x0006c750 supplies task and four-HRESULT output; body copies per-user change results. |
| `0x003a0671` | XONLINE | 4 | `XOnlineGetLogonUsers` | confirmed | 0 / 0 | Returns current user-array pointer; 0x0006bff0 validates per-user results. |
| `0x003a067c` | XONLINE | 2 | `XOnlineGetServiceInfo` | confirmed | 2 / 2 | Searches requested service IDs, returns service HRESULT and optional 12-byte endpoint; 0x0006bff0 checks availability. |
| `0x003a0687` | XONLINE | 1 | `XOnlineSaveLogonState` | likely | 1 / 1 | 0x0022387b saves session into launch data; body serializes users/services into supplied state. |
| `0x003a0692` | XONLINE | 1 | `XOnlineTitleUpdate` | confirmed | 1 / 1 | Update dialog callback 0x00236926 passes context 0 at 0x00236928. |
| `0x003a069d` | XONLINE | 1 | `XOnlineOfferingIsNewContentAvailable` | likely | 3 / 3 | 0x0008d4d0 starts new-offering check with bit filter/event/task output. |
| `0x003a06a8` | XONLINE | 1 | `XOnlineMatchSessionCreate` | confirmed | 8 / 8 | 0x0008fcc0 creates session with filled/open slots and attribute list; public signature matches eight stack words. |
| `0x003a06b7` | XONLINE | 1 | `XOnlineMatchSessionUpdate` | confirmed | 9 / 10 | 0x0008fe20 passes session ID by value plus slot counts and attributes; wrapper cleans 40 bytes. |
| `0x003a06e7` | XONLINE | 1 | `XOnlineMatchSessionGetInfo` | confirmed | 3 / 3 | 0x0008fd90 reads session ID and exchange key from completed creation task. |
| `0x003a06f2` | XONLINE | 1 | `XOnlineMatchSessionDelete` | likely | 3 / 4 | 0x00090020 deletes advertised session; by-value 8-byte session ID plus event/task. |
| `0x003a0710` | XONLINE | 1 | `XOnlineMatchSessionFindFromID` | likely | 3 / 4 | 0x000900e0/0x000900ff looks up one session ID; same four-word ABI as Delete. |
| `0x003a072e` | XONLINE | 1 | `XOnlineMatchSearch` | confirmed | 7 / 7 | 0x0008fa80 supplies procedure, result limit, attributes, result buffer size, event/task. |
| `0x003a073d` | XONLINE | 2 | `XOnlineMatchSearchGetResults` | confirmed | 3 / 3 | Both 0x0008fb30 and 0x00090160 retrieve result pointer array/count. |
| `0x003a0748` | XONLINE | 2 | `XOnlineMatchSearchParse` | likely | 4 / 4 | Same two callers parse each returned search result into title attributes. |
| `0x003a0753` | XONLINE | 1 | `XOnlineMatchSearchResultsLen` | confirmed | 3 / 3 | 0x0008fa80 calculates result capacity before starting search. |
| `0x003a075e` | XONLINE | 4 | `XOnlineTitleIdIsSameTitle` | confirmed | 1 / 1 | Body compares supplied title ID to current title; used by presence/friend UI. |
| `0x003a0769` | XONLINE | 1 | `XOnlineTitleIdIsSamePublisher` | confirmed | 1 / 1 | Body compares high 16 bits of supplied/current title IDs; friend compatibility check 0x0008d230. |
| `0x003a0774` | XONLINE | 2 | `XOnlineNotificationSetState` | confirmed | 5 / 6 | 0x0006c1d0 and 0x0006d040 pass controller, flags, by-value session ID, state length/data. |
| `0x003a0798` | XONLINE | 1 | `XOnlineGetNotificationEx` | likely | 3 / 3 | 0x0006d080 receives notification info and state flags for selected controller. |
| `0x003a07a3` | XONLINE | 1 | `XOnlineFriendsStartup` | likely | 2 / 2 | Post-logon 0x0008cc40 creates persistent friends task with event/task arguments. |
| `0x003a07ae` | XONLINE | 1 | `XOnlineFriendsEnumerate` | likely | 3 / 3 | 0x0008cce0 starts controller-specific enumeration task. |
| `0x003a07b9` | XONLINE | 1 | `XOnlineFriendsEnumerateFinish` | likely | 1 / 1 | Task disposer 0x0006b640 finishes enumeration before pump/close. |
| `0x003a07c4` | XONLINE | 1 | `XOnlineFriendsGetLatest` | likely | 3 / 3 | 0x0008cd60 supplies controller, capacity, friend buffer and consumes returned count. |
| `0x003a07cf` | XONLINE | 2 | `XOnlineFriendsGetTitleName` | likely | 4 / 4 | 0x0006b590/0x0008d1a0 supply title ID, language, character limit, wide buffer. |
| `0x003a07da` | XONLINE | 1 | `XOnlineFriendsRemove` | likely | 2 / 2 | 0x0008cdf0 supplies controller plus copied friend record. |
| `0x003a07e5` | XONLINE | 1 | `XOnlineFriendsGameInvite` | likely | 4 / 5 | 0x0008cfe0 passes controller, by-value session ID, one recipient and friend record. |
| `0x003a0806` | XONLINE | 1 | `XOnlineFriendsAnswerRequest` | likely | 3 / 3 | 0x0008ce40 maps UI answer to request-answer enum and passes friend/controller. |
| `0x003a0811` | XONLINE | 1 | `XOnlineFriendsAnswerGameInvite` | likely | 3 / 3 | 0x0008d050 maps UI invite answer and supplies friend/controller. |
| `0x003a081c` | XONLINE | 1 | `XOnlineFriendsJoinGame` | likely | 2 / 2 | UI function 0x002395dc passes controller/friend; two-word join-game entry. |
| `0x003a0827` | XONLINE | 1 | `XOnlineFriendsGetAcceptedGameInvite` | likely | 1 / 1 | 0x0006d210 passes accepted-invite output record; one-word result getter. |
| `0x003a0832` | XONLINE | 1 | `XOnlineMutelistGet` | likely | 6 / 6 | 0x00058b70 requests 250 mute entries with task, buffer and count outputs. |
| `0x003a0841` | XONLINE | 1 | `XOnlineMutelistStartup` | likely | 2 / 2 | 0x00058a90 creates global mute-list task with event/task output. |
| `0x003a084c` | XONLINE | 1 | `XOnlineMutelistAdd` | likely | 2 / 4 | 0x00058c10 passes controller and 12-byte XUID by value, then refreshes mute list. |
| `0x003a086e` | XONLINE | 1 | `XOnlineMutelistRemove` | likely | 2 / 4 | 0x00058c60 mirrors Add with removal operation and refresh. |
| `0x003a0890` | XONLINE | 1 | `XOnlineFeedbackSend` | likely | 6 / 8 | 0x0008d3d0 supplies controller, by-value XUID, feedback type (from its reason table 5, 6, 0, 1, 2, 3, 4, 12, 11, 10), a params block with a null string, event 0, task. Body 0x003b8efe sends the string as UTF-8 of at most 255 bytes; 0x003b8cbb skips repeats per controller. Decoded in live.md, "Client HLE: player feedback". |
| `0x003a08c0` | XONLINE | 1 | `XOnlineStatWrite` | likely | 4 / 4 | 0x000923c0 sends count/stat-spec array/event/task; body serializes stat values. |
| `0x003a08cb` | XONLINE | 1 | `XOnlineStatRead` | likely | 4 / 4 | 0x00092220 requests count/stat-spec array/event/task; body builds read request. |
| `0x003a08d6` | XONLINE | 1 | `XOnlineStatReadGetResult` | likely | 5 / 5 | 0x00092330 supplies task, count/specs and extra-buffer length/pointer. |
| `0x003a0c39` | XONLINE | 1 | `XOnlineStartup` | confirmed | 1 / 1 | 0x0008d840 calls after XNetStartup and WSAStartup; singleton creation, one startup-params pointer. |
| `0x003a0d36` | XONLINE | 1 | `XOnlineCleanup` | confirmed | 0 / 0 | 0x0008d970 calls before WSACleanup/XNetCleanup; releases XOnline singleton. |
| `0x003a11ae` | XONLINE | 1 | `XOnlineGetNatType` | confirmed | 0 / 0 | 0x0006b550 maps return values 1/2/3; body consults NAT-detection service 20. |
| `0x003a695e` | XONLINE | 1 | `XOnlinePresenceAdd` | likely | 4 / 4 | 0x0008c3a0 passes presence task, group, user count, XUID array. |
| `0x003a6969` | XONLINE | 1 | `XOnlinePresenceClear` | likely | 1 / 1 | 0x0008c550 clears presence task before closing it. |
| `0x003a6f9b` | XONLINE | 1 | `XOnlinePresenceSubmit` | likely | 1 / 1 | 0x0008c410 submits previously assembled presence task. |
| `0x003a6fa6` | XONLINE | 1 | `XOnlinePresenceGetLatest` | likely | 4 / 4 | 0x0008c470 receives group presence buffer with task/group/capacity arguments. |
| `0x003a8316` | XONLINE | 1 | `XOnlinePresenceInit` | likely | 3 / 3 | 0x0008c320 initializes per-controller presence task with event/task output. |
| `0x003a8321` | XONLINE | 1 | `XOnlineGameInviteSend` | likely | 8 / 9 | 0x0008f7e0 sends XUID recipient list, by-value session ID and message handle. |
| `0x003a834e` | XONLINE | 1 | `XOnlineGameInviteAnswer` | likely | 5 / 5 | 0x0008d0b0 supplies controller, answer-info record, answer enum, event/task. |
| `0x003a835d` | XONLINE | 1 | `XOnlineFriendsRequestEx` | likely | 5 / 7 | 0x0008f6e0 XUID branch passes message handle and by-value 12-byte recipient XUID. |
| `0x003a838a` | XONLINE | 1 | `XOnlineFriendsRequestByNameEx` | likely | 5 / 5 | Same wrapper name branch passes gamertag pointer instead of XUID value. |
| `0x003a8399` | XONLINE | 1 | `XOnlineTeamMemberRecruit` | likely | 7 / 11 | 0x0008f500 XUID branch supplies team and recipient XUID values, properties/message/event/task. |
| `0x003a83d4` | XONLINE | 1 | `XOnlineTeamMemberRecruitByName` | likely | 7 / 9 | Same wrapper name branch replaces recipient XUID with name pointer. |
| `0x003a8407` | XONLINE | 1 | `XOnlineTeamMemberAnswerRecruit` | likely | 5 / 7 | 0x000abfa0 supplies controller, by-value team XUID, answer enum, event/task. |
| `0x003a9d65` | XONLINE | 1 | `XOnlineMessageDetailsGetResultsProperty` | likely | 6 / 6 | 0x0008e9e0 reads typed property with buffer, size and attachment-flag outputs. |
| `0x003a9d74` | XONLINE | 1 | `XOnlineMessageDownloadAttachmentToMemory` | likely | 6 / 6 | 0x0008eab0 supplies details-task, property tag, buffer/size, event/task output. |
| `0x003a9d83` | XONLINE | 1 | `XOnlineMessageDownloadAttachmentToMemoryGetResults` | likely | 4 / 4 | 0x0008eb50 receives attachment pointer and received/total byte counts. |
| `0x003a9d8e` | XONLINE | 1 | `XOnlineMessageCreate` | likely | 7 / 8 | 0x0008f070 creates message with type, property counts, 64-bit context, flags, expiry, output handle. |
| `0x003a9db8` | XONLINE | 1 | `XOnlineMessageDestroy` | likely | 1 / 1 | 0x0008eee0 destroys message handle synchronously. |
| `0x003a9dc3` | XONLINE | 1 | `XOnlineMessageSetProperty` | likely | 5 / 5 | 0x0008f4b0 passes message, property tag, value length/pointer, attachment flags. |
| `0x003ab04a` | XONLINE | 1 | `XOnlineMessageEnumerate` | likely | 3 / 3 | 0x0008e750 receives controller message-summary buffer and count. |
| `0x003ab055` | XONLINE | 0 | `XOnlineMessageDelete` | likely | 3 / 3 | Tail transfer from 0x0008e920 passes controller, message ID, block-sender flag. |
| `0x003ab060` | XONLINE | 1 | `XOnlineMessageSend` | likely | 6 / 6 | 0x0008f8b0 supplies sender, message, recipient count/XUID array, event/task. |
| `0x003ab2a6` | XONLINE | 1 | `XOnlineMessageDetails` | likely | 6 / 6 | 0x0008e950 obtains message details while setting/clearing flags. |
| `0x003ab6a6` | XONLINE | 1 | `XOnlineStatWriteGetResult` | likely | 4 / 4 | 0x00092450 receives server-file handle, attachment-reference array and reference count. |
| `0x003ab6b1` | XONLINE | 1 | `XOnlineStorageCreateServerPath` | likely | 6 / 8 | 0x000b4120 supplies facility, 64-bit user/team IDs, wide filename, output path/length. |
| `0x003ab6db` | XONLINE | 1 | `XOnlineStorageDownload` | likely | 7 / 7 | 0x000b4810 downloads server path to local install directory, with flags/event/task. |
| `0x003ab6ea` | XONLINE | 1 | `XOnlineStorageDownloadToMemory` | likely | 8 / 8 | 0x000b4450 supplies facility/user/path, memory buffer/size, flags/event/task. |
| `0x003ab6f9` | XONLINE | 1 | `XOnlineStorageDownloadToMemoryGetResults` | likely | 6 / 6 | 0x000b45f0 reads data pointer, received/total sizes, owner and creation time. |
| `0x003ad149` | XONLINE | 1 | `XOnlineStorageUploadByServerPath` | likely | 8 / 9 | 0x000b4670 uploads directory with by-value FILETIME and server path. |
| `0x003ad176` | XONLINE | 1 | `XOnlineStorageUploadFromMemory` | likely | 9 / 10 | 0x000b42b0 uploads memory with by-value FILETIME and size/flags/event/task. |
| `0x003ad1a6` | XONLINE | 1 | `XOnlineStorageGetProgress` | confirmed | 4 / 4 | 0x000b49a0 reads percentage; body computes percentage and optional 64-bit numerator/denominator. |
| `0x003adc4c` | XONLINE | 1 | `XOnlineArbitrationCreateRoundID` | confirmed | 1 / 1 | 0x00073ae0 supplies 64-bit output; body fills eight random bytes until nonzero. |
| `0x003ae32b` | XONLINE | 1 | `XOnlineArbitrationRegister` | likely | 5 / 5 | 0x000924e0 registers arbitration ID with duration/flags/event/task. |
| `0x003ae33a` | XONLINE | 1 | `XOnlineArbitrationExtendRound` | likely | 4 / 4 | 0x000927b0 extends arbitration ID by supplied duration, event/task. |
| `0x003ae36a` | XONLINE | 1 | `XOnlineArbitrationReport` | likely | 7 / 7 | 0x000925c0 reports arbitration ID, stat procedures/report data, flags/event/task. |
| `0x003aeba5` | XONLINE | 1 | `XOnlineTeamEnumerateGetResults` | likely | 3 / 3 | 0x000abd00 receives team count and XUID output array. |
| `0x003aebb0` | XONLINE | 1 | `XOnlineTeamMembersEnumerateGetResults` | likely | 3 / 3 | 0x000ac1b0 receives member count and XUID output array. |
| `0x003afb80` | XONLINE | 1 | `XOnlineTeamCreate` | likely | 6 / 6 | 0x000abe60 supplies team/member properties, maximum members, event/task. |
| `0x003afb8f` | XONLINE | 1 | `XOnlineTeamDelete` | likely | 4 / 6 | 0x000abf10 supplies controller, by-value team XUID, event/task. |
| `0x003afbb9` | XONLINE | 1 | `XOnlineTeamMemberSetProperties` | likely | 6 / 10 | 0x000ac360 passes team/member XUID values and member properties. |
| `0x003afbf1` | XONLINE | 1 | `XOnlineTeamMemberRemove` | likely | 5 / 9 | 0x000ac050 passes team/member XUID values plus event/task. |
| `0x003afc26` | XONLINE | 1 | `XOnlineTeamEnumerateByUserXUID` | likely | 4 / 6 | 0x000abc70 enumerates teams for by-value user XUID with controller/event/task. |
| `0x003afc50` | XONLINE | 1 | `XOnlineTeamGetDetails` | likely | 3 / 5 | 0x000abdb0 supplies enumeration task, by-value team XUID, details output. |
| `0x003afc76` | XONLINE | 1 | `XOnlineTeamMembersEnumerate` | likely | 5 / 7 | 0x000ac110 supplies controller, by-value team XUID, flags/event/task. |
| `0x003afca3` | XONLINE | 1 | `XOnlineTeamMemberGetDetails` | likely | 3 / 5 | 0x000ac250 supplies enumeration task, by-value member XUID, details output. |
| `0x003b0254` | XONLINE | 1 | `XOnlineQuerySearch` | likely | 10 / 10 | 0x000b37f0 supplies dataset/procedure, paging, result specs, attributes, event/task. |
| `0x003b0263` | XONLINE | 1 | `XOnlineQuerySearchGetResults` | confirmed | 5 / 5 | 0x000b3970 reads total/returned counts and sized results; game diagnostic names this API. |
| `0x003cd0c0` | XNET | 1 | `XNetRandom` | confirmed | 2 / 2 | 0x0007ad80 passes buffer/size; body invokes random fill and returns zero. |
| `0x003cd0cb` | XNET | 1 | `XNetCreateKey` | confirmed | 2 / 2 | 0x0007a8a0 outputs 8-byte ID and 16-byte key; body fills both and normalizes ID flags. |
| `0x003cd0d6` | XNET | 2 | `XNetRegisterKey` | confirmed | 2 / 2 | 0x0007a920 supplies key-ID/key pointers; body inserts key registry entry. |
| `0x003cd0e1` | XNET | 6 | `XNetUnregisterKey` | confirmed | 1 / 1 | Paired with RegisterKey in key cleanup and shutdown; one key-ID pointer. |
| `0x003cd0ec` | XNET | 1 | `XNetXnAddrToInAddr` | confirmed | 3 / 3 | 0x0007adf0 supplies XNADDR/key-ID and receives IN_ADDR. |
| `0x003cd103` | XNET | 1 | `XNetTsAddrToInAddr` | confirmed | 4 / 4 | 0x000b3a70 supplies TSADDR/service/key-ID/output; game error diagnostic names this API. |
| `0x003cd147` | XNET | 7 | `XNetQosListen` | confirmed | 5 / 5 | 0x000731d0 supplies key-ID, listener data/size, bandwidth, flags; not a socket send/recv API. |
| `0x003cd156` | XNET | 1 | `XNetQosLookup` | confirmed | 12 / 12 | 0x0007b4c0 supplies address/key arrays and probe parameters; wrapper cleans 48 bytes. |
| `0x003cd172` | XNET | 3 | `XNetQosRelease` | confirmed | 1 / 1 | 0x0007b4c0/0x0007b5e0/0x0007b650 release returned QoS records. |
| `0x003cd17d` | XNET | 3 | `XNetGetTitleXnAddr` | confirmed | 1 / 1 | 0x0007a9b0 and 0x0008fd90 fill XNADDR; caller tests address-acquisition flags. |
| `0x003cd188` | XNET | 1 | `socket` | confirmed | 3 / 3 | 0x000b53e0 passes AF_INET, stream/datagram type and TCP/UDP protocol; body validates those domains. |
| `0x003cd193` | XNET | 2 | `closesocket` | confirmed | 1 / 1 | 0x000b4f50 calls after shutdown; body closes and removes socket entry. |
| `0x003cd19e` | XNET | 2 | `shutdown` | confirmed | 2 / 2 | 0x000b4f50 passes socket/how before close; two-word ABI. |
| `0x003cd1a9` | XNET | 2 | `ioctlsocket` | confirmed | 3 / 3 | 0x00092ae0/0x000b51b0 pass socket, ioctl command, value pointer; body matches signed socket API. |
| `0x003cd1b4` | XNET | 1 | `setsockopt` | confirmed | 5 / 5 | 0x000b4e70 passes option value/length; body reads supplied value and checks SOL_SOCKET/TCP. |
| `0x003cd1c3` | XNET | 1 | `getsockopt` | confirmed | 5 / 5 | 0x000b4e00 passes option-value and in/out length pointers; body writes output buffer. |
| `0x003cd1d2` | XNET | 1 | `bind` | confirmed | 3 / 3 | 0x000b4ed0 constructs local sockaddr then passes socket/address/length. |
| `0x003cd1dd` | XNET | 1 | `connect` | confirmed | 3 / 3 | 0x000b51b0 connects remote sockaddr and handles nonblocking WSA errors. |
| `0x003cd1e8` | XNET | 1 | `select` | confirmed | 5 / 5 | 0x000b52b0 builds fd_sets, passes timeout and checks set membership afterwards. |
| `0x003cd211` | XNET | 1 | `recv` | confirmed | 4 / 4 | 0x000b4fa0 passes socket, receive buffer, length, flags and checks received bytes. |
| `0x003cd22b` | XNET | 1 | `recvfrom` | confirmed | 6 / 6 | 0x000b5060 supplies writable source sockaddr and in/out source length. |
| `0x003cd23a` | XNET | 1 | `send` | confirmed | 4 / 4 | 0x000b5000 passes socket, payload, length and flags. |
| `0x003cd254` | XNET | 1 | `sendto` | confirmed | 6 / 6 | 0x000b5110 supplies destination sockaddr and its length after payload arguments. |
| `0x003cd32d` | XNET | 1 | `XNetInAddrToXnAddr` | confirmed | 3 / 3 | 0x0007ab60 supplies IN_ADDR and receives XNADDR plus key-ID. |
| `0x003cd344` | XNET | 4 | `XNetUnregisterInAddr` | confirmed | 1 / 1 | Address cleanup after service/query registration; 0x000b3b10 unregisters before key removal. |
| `0x003cd34f` | XNET | 1 | `XNetConnect` | confirmed | 1 / 1 | 0x0007acc0 starts secure-address connection for IN_ADDR. |
| `0x003cd35a` | XNET | 1 | `XNetGetConnectStatus` | confirmed | 1 / 1 | 0x0007acf0 reads secure-address connection state; uninitialized return is LOST state 3. |
| `0x003cd365` | XNET | 1 | `XNetQosServiceLookup` | confirmed | 3 / 3 | 0x0007b5e0 supplies flags/event/QoS-output; used by sign-in completion path 0x002bb04e. |
| `0x003cd576` | XNET | 1 | `XNetStartup` | confirmed | 1 / 1 | One startup-params pointer; 0x0008d8c7 checks zero before Winsock startup. |
| `0x003cd58d` | XNET | 1 | `WSAStartup` | confirmed | 2 / 2 | Version 0x0002 and WSADATA output at 0x0008d8db; zero required. |
| `0x003cd67b` | XNET | 1 | `XNetCleanup` | confirmed | 0 / 0 | XNet startup rollback and shutdown; distinct from WSACleanup by initializer mode flag. Also tail-jumped from 0x0008d9de. |
| `0x003cd688` | XNET | 2 | `WSACleanup` | confirmed | 0 / 0 | Winsock startup rollback and shutdown; paired with WSAStartup. |
| `0x003cd718` | XNET | 17 | `WSAGetLastError` | confirmed | 0 / 0 | Tailcalls thread-local last-error getter 0x002d1d3e; 17 socket-error call sites. |
| `0x003cd71d` | XNET | 2 | `__WSAFDIsSet` | confirmed | 2 / 2 | 0x000b52b0 passes socket/fd_set after select; body scans count and socket entries. |
| `0x003ce17b` | XNET | 1 | `htonl/ntohl` | confirmed | 1 / 1 | One 32-bit value, byte-order reversal; indistinguishable shared host/network conversion implementation. |
| `0x003ce184` | XNET | 3 | `htons/ntohs` | confirmed | 1 / 1 | One 16-bit value, byte-order reversal; both endian aliases share this operation. |

## Sign-in as driven by the game

Game function names below are descriptive labels or names from the independent decompilation, not renamed Ghidra symbols. The addresses and branches were checked against this XBE. Game functions commonly use register arguments; the public API table above is stdcall.

### 1. Bring up transport when the link is active

`0x0008d690` initializes transport state and calls `XNetGetEthernetLinkStatus` (`0x0036c47d`) at `0x0008d6b0`. It tests **bit 0**, then tail-enters startup `0x0008d840` only if active. The recurring update `0x0008d7e0` repeats the link test at `0x0008d7eb`: transitions up call startup; transitions down call shutdown `0x0008d970`.

Startup `0x0008d840` returns immediately if already started. Otherwise its order is:

1. Call link status again at `0x0008d8bd`. **This particular return value is ignored**; the preceding initialization/update checks supply the link gate.
2. `XNetStartup` (`0x003cd576`) at `0x0008d8c7`, with a 12-byte startup-parameter structure. Any nonzero return aborts.
3. `WSAStartup` (`0x003cd58d`) at `0x0008d8db`, requesting **0x0002 (Winsock 2.0)** and supplying WSADATA. Any nonzero return calls `XNetCleanup` and aborts.
4. `XOnlineStartup` (`0x003a0c39`) at `0x0008d8ed`, with a pointer to a zeroed startup-parameter word. Any nonzero return calls `WSACleanup`, then `XNetCleanup`, and aborts.
5. Mark transport started (`0x004d8b19`), initialize title-address state through `0x0007ae70` / `0x0007a9b0`, initialize the QoS record pool, and invoke registered startup callbacks.

`0x0007a9b0` calls `XNetGetTitleXnAddr` at `0x0007aa13`. **0 means pending**, **1 means no address/uninitialized**. Other values allow it to cache XNADDR; bits `0x1c` distinguish an acquired static/DHCP/PPPoE IP. The observed XNADDR size is 36 bytes. Returning zero forever will keep acquisition pending.

Shutdown `0x0008d970` invokes shutdown callbacks, unregisters registered keys, clears started state, then calls `XOnlineCleanup` at `0x0008d9d3`, `WSACleanup` at `0x0008d9d8`, and tail-jumps to `XNetCleanup` at `0x0008d9de`.

### 2. Enumerate stored accounts and choose a user

The account-selection UI `0x0024af1a` calls game wrapper `0x0006c800` at `0x0024afe6`; lookup-by-name `0x0024ab0c` calls the same wrapper at `0x0024ab3c`.

`0x0006c800` calls `XOnlineGetUsers` (`0x003a0636`) at `0x0006c811`, passing a user buffer and a **count output**. A negative HRESULT makes the wrapper return count zero; a nonnegative HRESULT returns the count written by the API. The count stack slot is not initialized by the wrapper. Do not interpret it as an input capacity merely because a reworked header comment describes one: the retail callee clears the count and initializes a fixed 16-user buffer.

The observed `XONLINE_USER` stride is **0x70 (112) bytes**. Relevant fields are user ID at `+0`, user flags at `+8`, gamertag at `+0x0c`, and per-user HRESULT at `+0x6c`. The logon array always has four controller slots; unassigned slots are zeroed.

### 3. Create the persistent logon task

Sign-in UI method `0x0024b824` calls `0x0006c8b0` at `0x0024b83f`, passing the controller and selected XONLINE_USER. The game requires both transport-initialized (`0x004d8b18`) and transport-started (`0x004d8b19`) flags.

When there is no existing logon task (`0x00467214 == -1`), `0x0006c8b0` builds the four-slot user array and puts the selected account in its controller slot. It creates a game task and calls:

`XOnlineLogon(users, service_ids, 13, NULL, &sdk_task)`

at `0x0006c948`, targeting `0x003a0641`. The service list is assembled by `0x0006c7a0` from the game's service descriptors at `0x00467178`. The retail descriptor table is not reproduced here.

The immediate return is tested with **HRESULT >= 0**, not against the eventual connected status. A negative result disposes the game task and resets its global index to `-1`; the sign-in method takes its failure callback. Success stores a persistent **type-0 game task**, whose `+0x0c` field contains the separate opaque SDK task handle.

The actual XOnline logon body `0x003a4fd5` independently checks link bit 0 at `0x003a5007`; no cable produces `XONLINE_E_LOGON_NO_NETWORK_CONNECTION` (`0x80151000`). It also has immediate not-initialized, task-busy and out-of-memory paths. The game wrapper does not route those immediate HRESULTs individually through the later connection-result classifier.

When already signed in, the alternate branch calls `XOnlineChangeLogonUsers` (`0x003a065b`) at `0x0006ca90`, with a replacement four-user array, null event and task output. User removal also uses this API at `0x0006cd05`. These are **type-1 game tasks**; they are distinct from the persistent type-0 connection task.

### 4. Pump the task, then read the connection result

Game update `0x0006b4a0` dispatches live tasks through `0x0006c560`. The type-0 path is `0x0006c2a0`:

1. `0x0006c670` calls `XOnlineTaskContinue` (`0x003a0620`) at `0x0006c6ab` when the task and transport are usable.
2. `0x0006cd50` calls **`XOnlineLogonTaskGetResults`** (`0x003a0650`) at `0x0006cdb4`. This reads the cached connection result separately from pumping.
3. Game result **0** keeps waiting. Game result **1** enters post-logon setup once, then keeps the logon task alive and pumped. Any other result marks it failed/finished.

The persistent logon handle must survive connection establishment. Closing it on successful sign-in would sign the users out. Game disposer `0x0006b640` eventually calls `XOnlineTaskClose` (`0x003a062b`) at `0x0006b6cb` and clears the SDK handle.

For ordinary asynchronous tasks, `0x0006c450` treats `XONLINETASK_S_RUNNING = 0` and `XONLINETASK_S_RUNNING_IDLE = 0x001500f2` as still active, `XONLINETASK_S_RESULTS_AVAIL = 0x001500f1` as results available, and other nonnegative results, including `XONLINETASK_S_SUCCESS = 0x001500f0`, as finished. Negative results finish with failure. The dedicated logon task has the special handling above.

The change-users path `0x0006c360` branches on `XOnlineTaskContinue` directly: **0** and **0x001513f0** (`COMMIT_USER_CHANGE`) continue; **0x001513f1** (`USER_CHANGE_COMPLETE`) runs post-logon validation and finishes successfully if it passes. **0x80151300** (`CHANGE_USER_FAILED`) calls `0x0006c750`, which invokes `XOnlineChangeLogonUsersTaskGetResults` (`0x003a0666`) at `0x0006c75f` to inspect four per-user HRESULTs. It recognizes the message warning and account-management error there.

### 5. Check users and all requested services

On connection success, `0x0006c2a0` calls post-logon validation `0x0006bff0` at `0x0006c323`.

- `0x0006bff0` calls `XOnlineGetLogonUsers` (`0x003a0671`) at `0x0006c012`. A null pointer fails validation with game result 10.
- `0x0006c6c0` checks the four 112-byte records. For a nonzero user ID, `hr == 0` is ordinary success; `hr == 0x001512f0` records a message warning; `hr == 0x80151200` returns the account-management result 2 and aborts this validation. Other per-user values are not comprehensively classified here; the selected user's final UI check still requires `hr >= 0`.
- At `0x0006c050`, `0x0006bff0` calls `XOnlineGetServiceInfo(service_id, NULL)` (`0x003a067c`) for each of the **13 requested services**. A nonnegative result marks that service available. A negative result marks it unavailable; if its descriptor's required flag is nonzero, validation fails with generic game result 10. Optional-service failures allow the loop to continue.
- It publishes notifications through `0x0006c1d0`, calling `XOnlineNotificationSetState` (`0x003a0774`) at `0x0006c269` for occupied, named, nonnegative-status, non-guest users. The call has a by-value XNKID and a four-byte state payload.
- It then starts friends (`0x0008cc40` -> `0x003a07a3`) and the new-content check (`0x0008d4d0` -> `0x003a069d`). Other callbacks also run; this is not a proof that every downstream service task can be omitted.

A second service-info use at `0x0006d24a` requests the endpoint for NAT-detection service **20**, then unregisters the old secure IN_ADDR through `XNetUnregisterInAddr` at `0x0006d258` when successful.

### 6. Finish the sign-in screen, including QoS

The sign-in screen callback is `0x0024b869`, registered by `0x0024b824`. It calls `0x002bb04e`, which, after game logon result 1, starts the **service QoS lookup** via `0x0007b5e0` -> `XNetQosServiceLookup` (`0x003cd365`) at `0x0007b5f7`, with flags 0, event 0 and an XNQOS output pointer.

`0x0007b6c0` polls the returned record: its pending count at `XNQOS + 4` must reach **zero** before completion. `0x002bb0e9` reads the result, may consult `XOnlineGetNatType` through `0x0006b550`, and releases QoS through `0x0007b650` -> `XNetQosRelease`. An HLE that signs in successfully but makes QoS startup fail can repeatedly retry this step; provide a valid record that completes, not only successful logon APIs.

The screen then reads `0x0006cd50` and copies `XOnlineGetLogonUsers` through `0x0006ced0`. To take success, it requires **game result 1**, selected user's **nonzero ID**, **nonempty gamertag**, and **nonnegative per-user HRESULT**. It calls success method `0x0024b971`, which updates the profile/gamertag and signed-in state. The persistent connection task is retained beyond the screen.

### 7. Failure routing and dashboard actions

The connection-result classifier at `0x0006cd50` produces the following **derived control-flow mapping**, not a copied retail lookup table. The common UI handler is `0x001486b8`, called both from the sign-in progress path (`0x002bb0b9`) and recurring UI update (`0x00149594`).

| SDK result | Game result | Observed UI/action |
|---|---:|---|
| `0x00000000` / `XONLINETASK_S_RUNNING` | 0 | Wait; no error dialog. |
| `0x001510f0` / `XONLINE_S_LOGON_CONNECTION_ESTABLISHED` | 1 | Validate users/services and finish sign-in; no error dialog. |
| `0x80151200` / account requires management | 2 | Dialog `0x27`; first action opens dashboard online menu through `0x00236937`. |
| `0x80151002` / update required | 3 | Dialog `0x24`; first action `0x00236926` calls `XOnlineTitleUpdate(0)` at `0x00236928`. |
| `0x80151001` / cannot access service | 4 | Dialog `0x23`; first action opens dashboard network configuration through `0x00236917`. |
| `0x80151004` / connection lost | 5 | Dialog `0x38`; normally offers the same network-configuration action. Transport becoming unusable also yields game result 5. |
| `0x80151006` / invalid user | 6 | Dialog `0x9a`; first action opens dashboard online menu through `0x00236937`. |
| `0x80151005` / kicked by duplicate logon | 7 | Dialog `0x26`; acknowledgement signs out/returns to main menu through `0x00236953`. |
| `0x80151003` / servers too busy | 8 | Dialog `0x25`; acknowledgement signs out/returns to main menu through `0x00236953`. |
| Other result / invalid handle / required-service failure | 10 | Generic dialog `0x38`, normally offering network configuration. |
| `0x001512f0` / user has message, if returned as the overall logon result | 10 | Sets message flag `0x0050944f`, then falls into generic failure. Normally this warning belongs in the per-user record. |

For game result 2 or 6, the handler's `keep` argument controls whether a second/cancel callback is absent or returns to the main menu. Result 9 also shares dialog `0x38`, but this connection-result classifier does not emit 9. For dialog `0x38` during an active game, the handler can substitute dialog `0xb8` and an in-game quit action; it does not always expose a dashboard action in that context.

Dashboard launches are **user dialog actions**, not unconditional jumps immediately on receiving an HRESULT. Network callback `0x00236917` and online-menu callback `0x00236937` call game dashboard-launch wrapper `0x002238f4`. It constructs launch data, calls the XAPI launch entry `0x002d07f2` with a null image name at `0x0022396f`, and does not return. The derived launch reasons are **6 for network configuration** and **9 for the online menu**. Cancel/acknowledge callback `0x00236953` signs out and returns to the game's main menu. Update callback `0x00236926` has its own `XOnlineTitleUpdate` boundary call; the library's update/launch machinery contains the `xonlinedash.xbe` references.

A successful sign-in with the **per-user** message warning sets `0x0050944f` in `0x0006bff0`; success method `0x0024b971` then shows dialog `0x28` offering the dashboard online-menu callback. Return `hr == 0` for a simple offline account without that warning.

## False positives in the supplied raw scan

| Raw target | Claimed section | Raw site | Why it is not a boundary call |
|---|---|---|---|
| `0x003ad222` | XONLINE | `0x0040ae35` | Inside the instruction at `0x0040ae33`, which loads a stack local. The actual following call begins at `0x0040ae36` and targets XPP helper `0x0040a85e`. |
| `0x003c9ae4` | XONLINE | `0x004036f7` | Inside the instruction at `0x004036f6`, which combines registers. The following real call begins at `0x004036f8` and targets D3D helper `0x00403360`. |
| `0x003dbc19` | XNET | `0x002d5691` | Inside the instruction at `0x002d568f`, which restores the stack pointer from a frame local. No call starts at the raw site. |

The other 182 supplied sites decode as direct calls to the listed targets when aligned from independently discovered containing functions. No additional false call sites were found. Existing function discovery folds a few small thunks into neighbors; public entry addresses and ABI were checked directly, rather than treating every discovered-function label as authoritative. For example, the update callback starts at `0x00236926`; a speculative containing-function seed at `0x00236925` is not its real entry.

## What this implies for the first HLE milestone

- Hook the **public target addresses** in the inventory. Most public entries load the real library singleton into ECX and tail-transfer to a `CXo`/`CXn` body. An HLE public-entry override can own its own handles/state; it need not recreate the singleton merely to execute these thunks. Internal signature hits, such as socket body `0x003cf777`, are not the game-facing socket entry `0x003cd188`.
- Supply link-active state, successful XNet/Winsock/XOnline startup, a nonpending title address, one stored account, and a persistent logon task. Distinguish the **game task index**, **SDK task handle**, and **guest user-array pointer**.
- Implement both task pumping and the **separate logon-result getter**, plus change-users results, stable logged-on-user memory and service-info queries. Use the 112-byte user layout the game observes. `XOnlineGetUsers`'s count is output-only on this path.
- Return zero from immediate startup/logon/service-info calls and ordinary per-user success; return `0x001510f0` as the established connection result. Keep the connection task alive. General task success is `0x001500f0`, which is a different value and role.
- Include QoS service lookup/release with eventual zero pending count. Friends/new-content/presence work begins after validation and also needs deliberate HLE behavior. The full set of downstream tasks required to keep every later screen healthy has not been dynamically exercised.
- Preserve Winsock's `SOCKET_ERROR`/`INVALID_SOCKET` plus last-error behavior. `0x003cd718` is `WSAGetLastError`; the heavily used `0x003cd0e1` / `0x003cd147` entries are key removal / QoS listen, not send/receive.

## Evidence sources and limits

Primary evidence is read-only decoding of `/Users/afr/Source/github.com/0xdeafcafe/h2recomp/disc/default.xbe`, starting from function addresses emitted by the **already-built** analyzer and checking calls, public thunks, stack cleanup and conditional branches. No build or game execution was performed. No Ghidra rename, retype, analysis or save command was issued. The resident project reported just one analyzed function, so Ghidra's read-only decompiler could not resolve the requested entries; direct decoding was used instead.

Public references used for names and declarations, with retail behavior taking precedence over comments:

- [RXDK public XOnline header, pinned revision](https://github.com/Team-Resurgent/RXDK-SDK/blob/1dc30011768cf75366428f23380e099afa4dce0d/include/xonline.h): API names, argument shapes, public status constants. Its rewritten explanatory comments are not treated as proof of retail behavior.
- [RXDK public Winsock/XNet header, same revision](https://github.com/Team-Resurgent/RXDK-SDK/blob/1dc30011768cf75366428f23380e099afa4dce0d/include/winsockx.h): socket/XNet prototypes and public record definitions.
- [Independent Halo 2 decompilation, pinned revision](https://github.com/kirklandsig/halo2-decompiled/tree/69217dcac48cffad7624f884d3a76cd7eb01d7c2/src), especially [online tasks](https://github.com/kirklandsig/halo2-decompiled/blob/69217dcac48cffad7624f884d3a76cd7eb01d7c2/src/online_tasks.cpp), [transport](https://github.com/kirklandsig/halo2-decompiled/blob/69217dcac48cffad7624f884d3a76cd7eb01d7c2/src/transport.cpp), [dialog routing](https://github.com/kirklandsig/halo2-decompiled/blob/69217dcac48cffad7624f884d3a76cd7eb01d7c2/src/unknown_147f6d.cpp) and [callbacks](https://github.com/kirklandsig/halo2-decompiled/blob/69217dcac48cffad7624f884d3a76cd7eb01d7c2/src/dialog_callbacks.cpp): useful caller-name and API-name cross-checks. The report does not copy their implementations or data tables.
- [Public OnlineDash symbol names](https://github.com/Cxbx-Reloaded/sym2idc/blob/master/FFFE0000_4C0590E3_Xbox%20OnlineDash.sym): older related SDK name/decoration cross-checks only; its addresses are not Halo 2 addresses.

The existing XbSymbolDatabase matcher was also run read-only for leads. Its small XOnline/XNet match set agreed with the checked logon/match/startup/socket bodies; no signature data was copied. The 68 likely rows still need individual semantic confirmation before binding production implementations. Exact `htonl` versus `ntohl`, and `htons` versus `ntohs`, names cannot be distinguished from their shared operation; the combined names are intentional. This inventory does not claim to discover every possible address-taken, indirect or cross-section library entry beyond the verified direct calls, the tail-jump audit, and the additional link-status entry.


## Direct-site address ledger

This address-only ledger makes the counts reviewable against the supplied scan. Values after an arrow are source instruction addresses, not bytes. `tail` sites are separate from the table's direct-call counts.

- `0x0036c47d` `XNetGetEthernetLinkStatus` -> `0x0008d6b0`, `0x0008d7eb`, `0x0008d8bd`.
- `0x003a0620` `XOnlineTaskContinue` -> `0x0006c6ab`.
- `0x003a062b` `XOnlineTaskClose` -> `0x0006b6cb`.
- `0x003a0636` `XOnlineGetUsers` -> `0x0006c811`.
- `0x003a0641` `XOnlineLogon` -> `0x0006c948`.
- `0x003a0650` `XOnlineLogonTaskGetResults` -> `0x0006cdb4`.
- `0x003a065b` `XOnlineChangeLogonUsers` -> `0x0006ca90`, `0x0006cd05`.
- `0x003a0666` `XOnlineChangeLogonUsersTaskGetResults` -> `0x0006c75f`.
- `0x003a0671` `XOnlineGetLogonUsers` -> `0x0006c012`, `0x0006c6de`, `0x0006cedc`, `0x0008c67d`.
- `0x003a067c` `XOnlineGetServiceInfo` -> `0x0006c050`, `0x0006d24a`.
- `0x003a0687` `XOnlineSaveLogonState` -> `0x002238ab`.
- `0x003a0692` `XOnlineTitleUpdate` -> `0x00236928`.
- `0x003a069d` `XOnlineOfferingIsNewContentAvailable` -> `0x0008d51f`.
- `0x003a06a8` `XOnlineMatchSessionCreate` -> `0x0008fd25`.
- `0x003a06b7` `XOnlineMatchSessionUpdate` -> `0x0008fea4`.
- `0x003a06e7` `XOnlineMatchSessionGetInfo` -> `0x0008fdef`.
- `0x003a06f2` `XOnlineMatchSessionDelete` -> `0x00090089`.
- `0x003a0710` `XOnlineMatchSessionFindFromID` -> `0x00090121`.
- `0x003a072e` `XOnlineMatchSearch` -> `0x0008faea`.
- `0x003a073d` `XOnlineMatchSearchGetResults` -> `0x0008fba7`, `0x000901e5`.
- `0x003a0748` `XOnlineMatchSearchParse` -> `0x0008fc5c`, `0x00090282`.
- `0x003a0753` `XOnlineMatchSearchResultsLen` -> `0x0008fad4`.
- `0x003a075e` `XOnlineTitleIdIsSameTitle` -> `0x0008c28e`, `0x0008c541`, `0x0008cf0e`, `0x0008d194`.
- `0x003a0769` `XOnlineTitleIdIsSamePublisher` -> `0x0008d27a`.
- `0x003a0774` `XOnlineNotificationSetState` -> `0x0006c269`, `0x0006d074`.
- `0x003a0798` `XOnlineGetNotificationEx` -> `0x0006d0d3`.
- `0x003a07a3` `XOnlineFriendsStartup` -> `0x0008ccb0`.
- `0x003a07ae` `XOnlineFriendsEnumerate` -> `0x0008cd27`.
- `0x003a07b9` `XOnlineFriendsEnumerateFinish` -> `0x0006b6a5`.
- `0x003a07c4` `XOnlineFriendsGetLatest` -> `0x0008cddb`.
- `0x003a07cf` `XOnlineFriendsGetTitleName` -> `0x0006b59d`, `0x0008d1da`.
- `0x003a07da` `XOnlineFriendsRemove` -> `0x0008ce2f`.
- `0x003a07e5` `XOnlineFriendsGameInvite` -> `0x0008d03a`.
- `0x003a0806` `XOnlineFriendsAnswerRequest` -> `0x0008ce97`.
- `0x003a0811` `XOnlineFriendsAnswerGameInvite` -> `0x0008d098`.
- `0x003a081c` `XOnlineFriendsJoinGame` -> `0x002396ca`.
- `0x003a0827` `XOnlineFriendsGetAcceptedGameInvite` -> `0x0006d211`.
- `0x003a0832` `XOnlineMutelistGet` -> `0x00058bd1`.
- `0x003a0841` `XOnlineMutelistStartup` -> `0x00058ab8`.
- `0x003a084c` `XOnlineMutelistAdd` -> `0x00058c45`.
- `0x003a086e` `XOnlineMutelistRemove` -> `0x00058c95`.
- `0x003a0890` `XOnlineFeedbackSend` -> `0x0008d4a0`.
- `0x003a08c0` `XOnlineStatWrite` -> `0x00092411`.
- `0x003a08cb` `XOnlineStatRead` -> `0x000922f4`.
- `0x003a08d6` `XOnlineStatReadGetResult` -> `0x000923a8`.
- `0x003a0c39` `XOnlineStartup` -> `0x0008d8ed`.
- `0x003a0d36` `XOnlineCleanup` -> `0x0008d9d3`.
- `0x003a11ae` `XOnlineGetNatType` -> `0x0006b565`.
- `0x003a695e` `XOnlinePresenceAdd` -> `0x0008c3fa`.
- `0x003a6969` `XOnlinePresenceClear` -> `0x0008c59b`.
- `0x003a6f9b` `XOnlinePresenceSubmit` -> `0x0008c45b`.
- `0x003a6fa6` `XOnlinePresenceGetLatest` -> `0x0008c507`.
- `0x003a8316` `XOnlinePresenceInit` -> `0x0008c367`.
- `0x003a8321` `XOnlineGameInviteSend` -> `0x0008f852`.
- `0x003a834e` `XOnlineGameInviteAnswer` -> `0x0008d15b`.
- `0x003a835d` `XOnlineFriendsRequestEx` -> `0x0008f76c`.
- `0x003a838a` `XOnlineFriendsRequestByNameEx` -> `0x0008f790`.
- `0x003a8399` `XOnlineTeamMemberRecruit` -> `0x0008f609`.
- `0x003a83d4` `XOnlineTeamMemberRecruitByName` -> `0x0008f657`.
- `0x003a8407` `XOnlineTeamMemberAnswerRecruit` -> `0x000ac01f`.
- `0x003a9d65` `XOnlineMessageDetailsGetResultsProperty` -> `0x0008ea76`.
- `0x003a9d74` `XOnlineMessageDownloadAttachmentToMemory` -> `0x0008eb15`.
- `0x003a9d83` `XOnlineMessageDownloadAttachmentToMemoryGetResults` -> `0x0008eb9b`.
- `0x003a9d8e` `XOnlineMessageCreate` -> `0x0008f139`.
- `0x003a9db8` `XOnlineMessageDestroy` -> `0x0008ef0c`.
- `0x003a9dc3` `XOnlineMessageSetProperty` -> `0x0008f4f7`.
- `0x003ab04a` `XOnlineMessageEnumerate` -> `0x0008e78a`.
- `0x003ab055` `XOnlineMessageDelete` -> no direct calls ; tail `0x0008e93b`.
- `0x003ab060` `XOnlineMessageSend` -> `0x0008f917`.
- `0x003ab2a6` `XOnlineMessageDetails` -> `0x0008e9a3`.
- `0x003ab6a6` `XOnlineStatWriteGetResult` -> `0x000924c5`.
- `0x003ab6b1` `XOnlineStorageCreateServerPath` -> `0x000b428d`.
- `0x003ab6db` `XOnlineStorageDownload` -> `0x000b48c0`.
- `0x003ab6ea` `XOnlineStorageDownloadToMemory` -> `0x000b4511`.
- `0x003ab6f9` `XOnlineStorageDownloadToMemoryGetResults` -> `0x000b4655`.
- `0x003ad149` `XOnlineStorageUploadByServerPath` -> `0x000b4726`.
- `0x003ad176` `XOnlineStorageUploadFromMemory` -> `0x000b436f`.
- `0x003ad1a6` `XOnlineStorageGetProgress` -> `0x000b49f0`.
- `0x003adc4c` `XOnlineArbitrationCreateRoundID` -> `0x00073afa`.
- `0x003ae32b` `XOnlineArbitrationRegister` -> `0x0009256d`.
- `0x003ae33a` `XOnlineArbitrationExtendRound` -> `0x0009282b`.
- `0x003ae36a` `XOnlineArbitrationReport` -> `0x00092711`.
- `0x003aeba5` `XOnlineTeamEnumerateGetResults` -> `0x000abd85`.
- `0x003aebb0` `XOnlineTeamMembersEnumerateGetResults` -> `0x000ac229`.
- `0x003afb80` `XOnlineTeamCreate` -> `0x000abec5`.
- `0x003afb8f` `XOnlineTeamDelete` -> `0x000abf71`.
- `0x003afbb9` `XOnlineTeamMemberSetProperties` -> `0x000ac437`.
- `0x003afbf1` `XOnlineTeamMemberRemove` -> `0x000ac0d2`.
- `0x003afc26` `XOnlineTeamEnumerateByUserXUID` -> `0x000abcd1`.
- `0x003afc50` `XOnlineTeamGetDetails` -> `0x000abe51`.
- `0x003afc76` `XOnlineTeamMembersEnumerate` -> `0x000ac173`.
- `0x003afca3` `XOnlineTeamMemberGetDetails` -> `0x000ac2f2`.
- `0x003b0254` `XOnlineQuerySearch` -> `0x000b388a`.
- `0x003b0263` `XOnlineQuerySearchGetResults` -> `0x000b39b3`.
- `0x003cd0c0` `XNetRandom` -> `0x0007ad90`.
- `0x003cd0cb` `XNetCreateKey` -> `0x0007a8ce`.
- `0x003cd0d6` `XNetRegisterKey` -> `0x0007a951`, `0x000b3a86`.
- `0x003cd0e1` `XNetUnregisterKey` -> `0x0005fb97`, `0x00072e22`, `0x0007a940`, `0x0008d9b9`, `0x000b3ad7`, `0x000b3b42`.
- `0x003cd0ec` `XNetXnAddrToInAddr` -> `0x0007ae1a`.
- `0x003cd103` `XNetTsAddrToInAddr` -> `0x000b3aa7`.
- `0x003cd147` `XNetQosListen` -> `0x0005fb57`, `0x0005fb7c`, `0x000731f9`, `0x00073231`, `0x0007324c`, `0x000732bb`, `0x000732dc`.
- `0x003cd156` `XNetQosLookup` -> `0x0007b558`.
- `0x003cd172` `XNetQosRelease` -> `0x0007b5a6`, `0x0007b63d`, `0x0007b697`.
- `0x003cd17d` `XNetGetTitleXnAddr` -> `0x0007aa13`, `0x0007ae9c`, `0x0008fdfc`.
- `0x003cd188` `socket` -> `0x000b543f`.
- `0x003cd193` `closesocket` -> `0x00092cbb`, `0x000b4f85`.
- `0x003cd19e` `shutdown` -> `0x00092caa`, `0x000b4f74`.
- `0x003cd1a9` `ioctlsocket` -> `0x00092b55`, `0x000b5250`.
- `0x003cd1b4` `setsockopt` -> `0x000b4eb6`.
- `0x003cd1c3` `getsockopt` -> `0x000b4e4f`.
- `0x003cd1d2` `bind` -> `0x000b4f1d`.
- `0x003cd1dd` `connect` -> `0x000b526b`.
- `0x003cd1e8` `select` -> `0x000b5382`.
- `0x003cd211` `recv` -> `0x000b4fcb`.
- `0x003cd22b` `recvfrom` -> `0x000b50ca`.
- `0x003cd23a` `send` -> `0x000b502b`.
- `0x003cd254` `sendto` -> `0x000b5158`.
- `0x003cd32d` `XNetInAddrToXnAddr` -> `0x0007ac10`.
- `0x003cd344` `XNetUnregisterInAddr` -> `0x0006d258`, `0x0007855a`, `0x000b3acb`, `0x000b3b2f`.
- `0x003cd34f` `XNetConnect` -> `0x0007acd7`.
- `0x003cd35a` `XNetGetConnectStatus` -> `0x0007ad04`.
- `0x003cd365` `XNetQosServiceLookup` -> `0x0007b5f7`.
- `0x003cd576` `XNetStartup` -> `0x0008d8c7`.
- `0x003cd58d` `WSAStartup` -> `0x0008d8db`.
- `0x003cd67b` `XNetCleanup` -> `0x0008d95c` ; tail `0x0008d9de`.
- `0x003cd688` `WSACleanup` -> `0x0008d957`, `0x0008d9d8`.
- `0x003cd718` `WSAGetLastError` -> `0x00092b66`, `0x00092cb3`, `0x00092cc4`, `0x000b4e58`, `0x000b4ebf`, `0x000b4f31`, `0x000b4f7d`, `0x000b4f8e`, `0x000b4fd6`, `0x000b5036`, `0x000b50d7`, `0x000b5163`, `0x000b5283`, `0x000b5288`, `0x000b5298`, `0x000b53c6`, `0x000b5462`.
- `0x003cd71d` `__WSAFDIsSet` -> `0x000b5394`, `0x000b53aa`.
- `0x003ce17b` `htonl/ntohl` -> `0x000b5d48`.
- `0x003ce184` `htons/ntohs` -> `0x000b5d54`, `0x00376bc5`, `0x00376be2`.
