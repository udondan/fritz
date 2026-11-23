# fritz

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/udondan/fritz/main/docs/banner-dark.webp">
  <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/udondan/fritz/main/docs/banner-light.webp">
  <img alt="fritz – CLI + MCP for your FRITZ!Box" src="https://raw.githubusercontent.com/udondan/fritz/main/docs/banner-light.webp">
</picture>

[![Release](https://img.shields.io/github/v/release/udondan/fritz?filter=fritz-v*&label=release)](https://github.com/udondan/fritz/releases/latest)
[![Docker](https://img.shields.io/docker/v/udondan/fritz?sort=semver&label=docker)](https://hub.docker.com/r/udondan/fritz)
[![CI](https://github.com/udondan/fritz/actions/workflows/test.yml/badge.svg?branch=main)](https://github.com/udondan/fritz/actions/workflows/test.yml)
[![codecov](https://codecov.io/gh/udondan/fritz/graph/badge.svg)](https://codecov.io/gh/udondan/fritz)
[![Maintainability](https://qlty.sh/gh/udondan/projects/fritz/maintainability.svg)](https://qlty.sh/gh/udondan/projects/fritz)
[![License](https://img.shields.io/github/license/udondan/fritz)](https://github.com/udondan/fritz/blob/main/LICENSE)

A command line tool and MCP server that lets you and your AI agents read the
status and settings of a FRITZ!Box router.

> [!NOTE]
> **Unofficial.** This project is not affiliated with, endorsed by or supported
> by FRITZ! GmbH (formerly AVM). FRITZ! and FRITZ!Box are trademarks of
> FRITZ! GmbH.

> [!WARNING]
> **Early release.** Reading works over TR-064 and the Smart Home REST API.
> Released builds cannot change anything; changing settings needs the cargo
> feature `write` and several explicit opt-ins (see [SECURITY.md](https://github.com/udondan/fritz/blob/main/SECURITY.md)).

## Contents

- [Installation](#installation)
- [Usage](#usage)
  - [Set up](#set-up)
  - [Status and internet](#status-and-internet)
  - [Hosts and mesh](#hosts-and-mesh)
  - [Wireless networks](#wireless-networks)
  - [Interfaces and counters](#interfaces-and-counters)
  - [Smart Home](#smart-home)
  - [Ports, telephony and capabilities](#ports-telephony-and-capabilities)
  - [Settings](#settings)
  - [Calls, phonebook and event log](#calls-phonebook-and-event-log)
  - [Any TR-064 action](#any-tr-064-action)
  - [JSON and YAML output](#json-and-yaml-output)
  - [Several devices and the environment](#several-devices-and-the-environment)
  - [Debug log](#debug-log)
  - [Cache](#cache)
  - [Shell completion](#shell-completion)
  - [Version and updates](#version-and-updates)
- [MCP server](#mcp-server)
  - [Docker](#docker)
- [Changes](#changes)
- [Crates](#crates)
- [Security](#security)
- [Contributing](#contributing)
- [License](#license)
- [Disclaimer](#disclaimer)

## Installation

```sh
brew install udondan/software/fritz   # macOS and Linux
cargo install fritz                   # from source
docker pull udondan/fritz             # see Docker below
```

The [releases](https://github.com/udondan/fritz/releases) also have binaries
for Linux (x86_64, aarch64, static), macOS and Windows, with the
[completion scripts](#shell-completion) in `completions/`; Homebrew installs
them for bash, zsh and fish. `SHA256SUMS.txt` lists the checksums of the
archives and is signed with the GPG key `F111A9BDD62F384A`:

```sh
gpg --verify SHA256SUMS.txt.asc SHA256SUMS.txt
sha256sum --check --ignore-missing SHA256SUMS.txt
```

The Docker image is signed with cosign by the release workflow:

```sh
cosign verify udondan/fritz \
  --certificate-identity-regexp '^https://github.com/udondan/fritz/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

## Usage

The examples show made-up data. Every command takes `--help`; the commands
that read the device also print JSON and YAML (see
[JSON and YAML output](#json-and-yaml-output)).

### Set up

`fritz` logs in with a user of its own. [docs/box-setup.md](https://github.com/udondan/fritz/blob/main/docs/box-setup.md)
shows how to create one on the FRITZ!Box with no more rights than needed.
Then save the address and the user to a profile:

```console
$ fritz config init --host 192.168.178.1 --user agent
saved the profile default to /home/me/.config/fritz/config.toml
```

`fritz` talks to the box over HTTPS on port 49443. The box has a self-signed
certificate, so trust it once; `fritz` then refuses any other certificate.
Compare the fingerprint with the certificate in the web interface of the box:

```console
$ fritz tls trust
https://192.168.178.1:49443/ presents the certificate
  3F:A1:77:0C:5D:E2:94:1B:C8:60:2E:7A:D3:19:B4:8F:06:CA:51:E9:72:3D:8B:A4:1F:C0:66:95:2E:B7:D8:4C
Compare it with the certificate in the web interface of the device.
Trust it? [y/N] y
the profile default trusts the certificate 3F:A1:77:0C:…:D8:4C of https://192.168.178.1:49443/
```

Store the password in the keychain of the system (Keychain on macOS,
Credential Manager on Windows, Secret Service on Linux):

```console
$ fritz auth login
Password of agent@192.168.178.1:
stored the password of agent@192.168.178.1 in the keychain
$ fritz config show
config       /home/me/.config/fritz/config.toml
profile      default
address      https://192.168.178.1:49443/
user         agent
certificate  3F:A1:77:0C:5D:E2:94:1B:C8:60:2E:7A:D3:19:B4:8F:06:CA:51:E9:72:3D:8B:A4:1F:C0:66:95:2E:B7:D8:4C
password     from the keychain
```

If the password is in a password manager already, `fritz` can ask it instead,
with a command that prints the password. The command is a list of arguments
and runs without a shell; `fritz` strips one trailing line break from its
output:

```sh
fritz config init --password-command '["op", "read", "op://Private/FRITZ!Box/password"]'
fritz config init --password-command 'pass show fritz'
```

The password comes from `FRITZ_PASSWORD`, else from the command in
`FRITZ_PASSWORD_COMMAND` or the profile, else from the keychain. The command
runs once per run, at most 30 seconds, and may ask on the terminal to unlock
the password manager. Errors name the command, never its output.

| Command | Does |
| --- | --- |
| `fritz config init` / `show` / `list` | Save `--host`, `--user`, `--fingerprint` and `--password-command` to a profile / show the settings in effect / list the profiles |
| `fritz tls fingerprint` / `trust` / `forget` | Print / trust / forget the certificate of the device |
| `fritz auth login` / `logout` / `status` | Store / remove the password in the keychain / tell where it comes from |

FRITZ!Repeaters have only a password: leave out `--user` for them.

### Status and internet

```console
$ fritz overview
Model       FRITZ!Box 7690
Firmware    285.08.25
Uptime      8d 19h 13m
Update      none
Two-factor  on
Internet    Connected over DSL (link Up), 2001:db8::1, for 9h 36m
Rights      BoxAdmin: readwrite, Phone: readwrite, HomeAuto: readwrite
```

`--system` adds the load of the device: CPU load, CPU temperature and memory
now, and the lowest, average and highest values of the last 24 hours. It is
experimental: the values come from a page of the web interface, which may
change with FRITZ!OS, and need the right to see the settings. Devices without
the page get a note instead, and the command does not fail. In JSON, the
values are in `system`, with `null` for what the device does not show, or
`system` is `null` and `system_note` tells why.

```console
$ fritz overview --system
...
System           experimental, read from the web interface
CPU load         8% (24 h: min 6%, avg 8.2%, max 15%)
CPU temperature  67 °C (24 h: min 62 °C, avg 65.7 °C, max 69 °C)
Memory           49% used, 11% cache, 40% free (24 h: min 48.9%, avg 49%, max 49%)

$ fritz internet
Status         Connected
Access         DSL
Link           Up
Provider       Example ISP
Address        2001:db8::1
DNS servers    2001:db8::2, 2001:db8::3
Connected for  9h 36m
Link speed     ↓ 276.3 Mbit/s  ↑ 45.9 Mbit/s
DSL            Up
DSL rate       ↓ 274.8 Mbit/s  ↑ 45.9 Mbit/s
DSL maximum    ↓ 276.3 Mbit/s  ↑ 45.9 Mbit/s
Noise margin   ↓ 7.0 dB  ↑ 8.0 dB
Attenuation    ↓ 14.0 dB  ↑ 10.0 dB
```

`--dsl` shows the DSL line in detail, for an unstable line or the provider's
support: standard and profile, rates, noise margin, attenuation and power,
the error counters in total (and since the last sync, where the device
reports them), whether and since when the line lost its signal, and the line
diagnosis the device runs on its own 5 minutes after a loss of signal, with
the estimated distance to a cable fault. Hints put the numbers into context,
like a noise margin under 6 dB. On a connection that is not DSL, it says so
and exits with 0. It only reads; it does not start a diagnosis.

```console
$ fritz internet --dsl
Status        Up
Enabled       yes
Synchronized  yes
Standard      VDSL, profile 35b
Data path     Fast
Rate          ↓ 274.8 Mbit/s  ↑ 45.9 Mbit/s
Maximum       ↓ 276.3 Mbit/s  ↑ 45.9 Mbit/s
Noise margin  ↓ 7.0 dB  ↑ 8.0 dB
Attenuation   ↓ 14.0 dB  ↑ 10.0 dB
Power         ↓ 514  ↑ 500
Link          PPPoE, Up
Signal lost   no
Diagnosis     none

ERRORS                    TOTAL
CRC                       203
CRC (exchange)            0
FEC                       36486673
FEC (exchange)            77148
...
Resyncs                   1
```

With `--watch` it reads again every `--interval` seconds and prints the
counters that grew, with a hint when CRC errors rise or the line
resynchronizes; `--output json` prints one line per interval.

```console
$ fritz internet --dsl --watch --interval 60
Status   Up
Changes  FEC +1204 in 60 s

Status   Up
Changes  CRC +3, FEC +982, Errored seconds +1 in 60 s
Hint     CRC errors rose by 3 in 60 s: data was lost on the line
```

`fritz firmware` shows more about updates than the overview: the version the
device offers, the kind of build, what automatic updates do and how the last
update went. The state is `available`, `none`, `unknown` (the device does not
tell, for example after a failed check) or `unsupported` (the device has no
update service). What the device does not report is `unknown`, never `none`.
It only reads what the device knows; it never asks the device to check for a
new version. The download URL is never shown. In JSON, all fields are
present, with `null` for what the device does not report.

```console
$ fritz firmware
Installed    285.08.25 (release)
Offered      none
Auto update  install all updates
Last update  successful, 2026-08-02 03:28, from 285.08.22
State        no update offered
```

### Hosts and mesh

`fritz hosts` lists every device the box knows, `--active` only those
connected now:

```console
$ fritz hosts --active
NAME    IP              MAC                ACTIVE  INTERFACE
laptop  192.168.178.20  02:00:5E:10:00:01  yes     802.11
phone   192.168.178.21  02:00:5E:10:00:02  yes     802.11
nas     192.168.178.10  02:00:5E:10:00:03  yes     Ethernet
tv      192.168.178.30  02:00:5E:10:00:04  yes     Ethernet
```

`--search` keeps the hosts whose name, IP or MAC address contains a text. A
name, `--mac` or `--ip` shows one host with all the box knows about it. A name
matches exactly first, ignoring case, then as part of a name; several matches
show the table. If nothing matches, `fritz` exits with code 1:

```console
$ fritz hosts --mac 02:00:5e:10:00:01
Name             laptop
IP               192.168.178.20
MAC              02:00:5E:10:00:01
Active           yes
Address source   DHCP
Interface        802.11
Speed            866 Mbit/s
Guest            no
Internet         granted
Attached to      fritzbox, WLAN 5 GHz ↓ 866.7 Mbit/s  ↑ 433.3 Mbit/s
```

```console
$ fritz mesh
NODE      IP              ROLE    MODEL                   MAC
fritzbox  192.168.178.1   master  FRITZ!Box 7690          02:00:5E:10:00:A0
repeater  192.168.178.40  slave   FRITZ!Repeater 1200 AX  02:00:5E:10:00:A1

FROM      TO        TYPE  INTERFACE  STATE      RATE
fritzbox  repeater  WLAN  AP:5G:0    CONNECTED  ↓ 1201.0 Mbit/s  ↑ 1201.0 Mbit/s
fritzbox  laptop    WLAN  AP:5G:0    CONNECTED  ↓ 866.7 Mbit/s  ↑ 433.3 Mbit/s
repeater  tv        LAN   LAN:1      CONNECTED  ↓ 0 kbit/s  ↑ 0 kbit/s
```

`--tree` shows which device is connected to which router or repeater:

```console
$ fritz mesh --tree
NODE          IP              ROLE    MODEL                   CONNECTION
fritzbox      192.168.178.1   master  FRITZ!Box 7690
├── repeater  192.168.178.40  slave   FRITZ!Repeater 1200 AX  WLAN 5 GHz ↓ 1201.0 Mbit/s  ↑ 1201.0 Mbit/s
│   └── tv    192.168.178.30                                  LAN
└── laptop    192.168.178.20                                  WLAN 5 GHz ↓ 866.7 Mbit/s  ↑ 433.3 Mbit/s
```

The IP addresses come from the host list, joined by the MAC addresses of
each node and its interfaces; the box itself has its address from its LAN
settings. A host that is not active keeps its last address, shown as
`192.168.178.30 (inactive)` and with `"active": false` in JSON, since the box
may give that address to another device. An unknown address is `-`, or
`null` in JSON. If the host list cannot be read, the mesh is shown without
the addresses of hosts, and a note on stderr says why.

`fritz powerline` lists the powerline adapters the box knows: name, model,
MAC address, whether the adapter is connected, and whether the box offers a
firmware update for it, with the result of the last update where the box
reports one. The box does not report data rates and firmware versions of the
adapters over TR-064. On a device without powerline support, the command
says so, and JSON has `"supported": false`. It never starts an update. The
energy saving of the adapters is in `fritz settings plc`.

```console
$ fritz powerline
NAME    MODEL                 MAC                ACTIVE  UPDATE
Office  FRITZ!Powerline 1260  02:00:5E:10:00:07  yes     none
Cellar  FRITZ!Powerline 540E  02:00:5E:10:00:08  no      available
```

### Wireless networks

```console
$ fritz wlan
SERVICE             SSID        TYPE   BAND     CHANNEL  STANDARD  STATUS    CLIENTS
WLANConfiguration1  Home               2.4 GHz  6        be        Up        2
WLANConfiguration2  Home               5 GHz    48       be        Up        1
WLANConfiguration3  Home-Guest  guest           6                  Up        1
```

The guest network is the one the device reports as such. `--clients` also
lists the clients of each network, with their names from the host table, the
signal strength in percent and the rate:

```console
$ fritz wlan --clients
WLANConfiguration1  Home  2.4 GHz  ch 6  be  Up  2 clients
  NAME     IP              MAC                SIGNAL  RATE
  phone    192.168.178.21  AA:BB:CC:00:00:01  71 %    144 Mbit/s
  printer  192.168.178.30  AA:BB:CC:00:00:02  52 %    72 Mbit/s

WLANConfiguration2  Home  5 GHz  ch 48  be  Up  1 client
  NAME    IP              MAC                SIGNAL  RATE
  laptop  192.168.178.20  AA:BB:CC:00:00:03  91 %    2401 Mbit/s

WLANConfiguration3  Home-Guest  guest  ch 6  Up  1 client
  NAME  IP              MAC                SIGNAL  RATE
        192.168.179.20  AA:BB:CC:00:00:04  64 %    86 Mbit/s
```

The device reports the clients of all networks in one list, read once: a
client belongs to the guest network by its guest flag, or else to the network
on its channel. Where there is no list, or where it disagrees with the number
of clients of a network, `fritz` asks for one client per call.

`--details` adds the security settings, the radio, WPS, the night time
switch-off and the packet statistics of each network. `fritz wlan <network>`
shows one network with its details, named by its service, its number or its
SSID. The text shows the encryption, channel, BSSID and packets, and the rest
only where it differs from the defaults; JSON has all fields. Weak settings
are hints, not errors: `open network`, `WEP`, `TKIP allowed`, `WPS enabled`
and `hidden SSID`. Keys and passphrases are not read here; only
`fritz wlan qr` reads a passphrase.

```console
$ fritz wlan 2
WLANConfiguration2  Home  5 GHz  ch 48  be  Up  1 client
  Security  WPA2/WPA3 mixed, AES
  Channel   48, automatic
  BSSID     AA:BB:CC:00:00:3A
  Packets   1200 sent, 3400 received
```

#### QR code to join a network

`fritz wlan qr <network>` shows a QR code that a phone scans to join a
network: `fritz wlan qr 1` for the first network, `fritz wlan qr guest` for
the guest network on any model. `guest`, in any case, always means the guest
network, also if another network has it as its SSID; that one can still be
named by its number or service. Other networks are named like above. There
is no default: without a name, `fritz wlan qr` lists the networks on stderr,
reads no password and fails, also without a terminal. It does the same if no
network fits the name, or if there is no guest network.

```console
$ fritz wlan qr
NUMBER  SERVICE             SSID        BAND     GUEST  STATUS
1       WLANConfiguration1  Home        2.4 GHz         on
2       WLANConfiguration2  Home        5 GHz           on
3       WLANConfiguration3  Home-Guest           guest  on
Error: name the network of the code, like: fritz wlan qr 1 or fritz wlan qr guest
```

With a network, it reads the password, and is the only command that does:
through a narrow path that only calls `GetSecurityKeys` of a
`WLANConfiguration` service and keeps only the passphrase. `fritz tr064 call`,
the capture and the MCP server still refuse it. WPA2, WPA3 and mixed networks become `WIFI:T:WPA;S:<ssid>;P:<password>;;`,
open and OWE networks `WIFI:T:nopass;S:<ssid>;;`, with `H:true;` for a hidden
SSID; `\`, `;`, `,`, `:` and `"` are escaped. WEP is not supported.

```console
$ fritz wlan qr guest


    █▀▀▀▀▀█ ▄  ▀▄▀▀▀█▀█▀▄▀ ▄▄ █▀▀▀▀▀█
    █ ███ █ █▄▀▄█ █ █ ▄▀█▀▄▀  █ ███ █
    █ ▀▀▀ █  ▀▄██▄██▀▄▄▄▀█▄█▀ █ ▀▀▀ █
    ▀▀▀▀▀▀▀ █ █ █▄▀▄█▄▀▄█▄▀▄█ ▀▀▀▀▀▀▀
    █   ▄ ▀ █  ██▄█ ▀█▀███▀ █▀█▄▄██▀▄
    ▀▄▄▀ █▀▄▄ ▄▄ █▀▄▄█▄▄█▄▀▀▀█▄▄███▄▄
    ██▀ ▄█▀▄█▄████ ▀ ▀▄▀ ▀ █▄▀█▀██▄ █
      ▀▄▀ ▀ ▄██▄▄▀▀▀█   █ ██▄  ██ ▄▄
    ██▄▀ ▀▀█▀▄   ▄█▀▀ █ █▀█▄█▀ █▀▀▀▄▀
     ███▀█▀▀▀▄ ▄   ██▀▀██▀█▄▄ ██▀▄  ▄
    ▄▀▀ ▄▄▀▄█ █ ▄█▄▀  ▀  ▀   ▄▄▀▄█▄▄▄
    █ ▀▄▄█▀▄█ ▄▄█▄▄▄▄█▀█▄▄ ▄▀▄▄▄▀▀▄█
    ▀▀▀▀  ▀ █▄  ▀▄  ▀▄█ ▄█▄██▀▀▀█▄▄▄▀
    █▀▀▀▀▀█  ▄▀ █▀▀▄▀█▄▄▀▄█▀█ ▀ █▄▄▀
    █ ███ █  ▀▀█  ▄▀▀ ▄ ▀▀▀▄█▀█▀██▄▀
    █ ▀▀▀ █  ▄▄▄█ ▀▀  ▄▀ ▀ ▄▀▄  ▄ ▄▀█
    ▀▀▀▀▀▀▀ ▀▀   ▀ ▀▀   ▀ ▀ ▀▀▀ ▀▀


Service   WLANConfiguration3
SSID      Home-Guest
Security  WPA
Hidden    no
Guest     yes
```

The code is printed with Unicode half blocks, dark on light, only to a
terminal. Without one, like in a pipe, `fritz` refuses before it asks the
device; write the code to a file instead, which only you can read:
`--out guest.png` or `--out guest.svg` (the format follows the extension, or
`--format text|png|svg`), with `--scale` pixels per module (8) and a border
of `--border` modules (4). `--out` is not offered on Windows, where `fritz`
cannot make the file readable only by you. `--show-password` also prints the
password below the code, only to a terminal.

### Interfaces and counters

`fritz interfaces` shows the LAN interface and the wireless networks with
their status, link, bytes and packets, and errors and discarded packets
where the device reports them (FRITZ!OS does not over TR-064). The LAN ports
come from the web interface; if it cannot be read, a note says why.

```console
$ fritz interfaces
INTERFACE                    NAME        STATUS    LINK                RECEIVED  SENT    PACKETS RECEIVED  PACKETS SENT  ERRORS  DISCARDED
LANEthernetInterfaceConfig1              Up        Auto, Auto duplex   876.4 GB  1.1 TB  709553279         1029439075
WLANConfiguration1           Home        Up        2.4 GHz, channel 6                    48211907          93127730
WLANConfiguration2           Home        Up        5 GHz, channel 48                     301947112         512033871
WLANConfiguration3           Home-Guest  Disabled  6 GHz, channel 37                     0                 0

PORT   CONNECTED  SPEED        MAXIMUM      ENERGY SAVING
LAN 1  yes        1000 Mbit/s  2500 Mbit/s  on
LAN 2  no                      1000 Mbit/s  on
```

`--watch` reads again every `--interval` seconds (default 5) and prints the
rates and the new errors since the read before, until interrupted or after
`--count` intervals. With `--output json`, each interval is one line
(NDJSON). A counter that falls is taken as wrapped around at 2^32 if it fit
in 32 bits, and as reset otherwise; a reset leaves the change out.

```console
$ fritz interfaces --watch --interval 10
INTERFACE                    NAME        STATUS    RECEIVED     SENT        PACKETS RECEIVED  PACKETS SENT  ERRORS  DISCARDED
LANEthernetInterfaceConfig1              Up        48.2 Mbit/s  2.1 Mbit/s  4211.6/s          1893.2/s
WLANConfiguration1           Home        Up                                 12.4/s            15.0/s
WLANConfiguration2           Home        Up                                 3920.8/s          2104.4/s
WLANConfiguration3           Home-Guest  Disabled                           0.0/s             0.0/s
```

### Smart Home

Read through the web interface, on FRITZ!OS 8.20 and later:

```console
$ fritz smarthome
NAME         CATEGORY    CONNECTED  SWITCH  TEMPERATURE  TARGET   POWER   ENERGY   BATTERY  OTHER
coffee       socket      yes        on      21.5 °C               23.4 W  1053 Wh           228.1 V
living room  thermostat  yes                20.5 °C      21.0 °C                   80%
hallway      lamp        yes        off                                                     level 25%
window       sensor      no                                                        10% low  humidity 55%, alert open
```

### Ports, telephony and capabilities

```console
$ fritz ports
UPnP  on
No port forwardings.

$ fritz telephony
NUMBER    TYPE   NAME
03012345  eVoIP

ACCOUNT  NUMBER    REGISTRAR        STATUS
1        03012345  sip.example.net  registered

ANSWERING MACHINE  NAME  ENABLED
0                  Home  no

HANDSET  NAME     MODEL         ACTIVE  UPDATE
1        Kitchen  FRITZ!Fon C5  no      no

$ fritz capabilities
Features    dsl, wlan, hosts, port_forwarding, upnp, parental_controls, telephony, dect, smarthome, myfritz
Rights      BoxAdmin: readwrite, Phone: readwrite, HomeAuto: readwrite
Two-factor  on
Services    37
```

`fritz users` lists the users of the device, your rights and how the login is
protected, with hints on weak settings like an active default password or a
login without password. No passwords are read; the list holds only names. If
your user may not read the list, it shows `not allowed` (`null` in JSON). The
MCP tool `fritz_users` hides the names unless `fritz mcp` runs with
`--personal`.

```console
$ fritz users
Users                   admin (last login), guest
Your rights             BoxAdmin: readwrite, Phone: readwrite, HomeAuto: readwrite
Login without password  no
Login by button         no
Default password        no
Two-factor              off
Hints                   two-factor confirmation off
```

### Settings

`fritz settings` reads settings that TR-064 doesn't offer, like DNS, IPv6,
DHCP and the ports, through the web interface. Secrets are hidden. Without
arguments it reads the common modules; name modules to read only those:

```console
$ fritz settings ipv6
ipv6.dhcpv6d_mode          dhcpv6d_mode_stateless
ipv6.enabled               1
ipv6.ip_prefix             2001:db8:1::
ipv6.ip_prefixlen          64
ipv6.mtu_override          1492
ipv6.radv_enabled          1
ipv6.ulamode               ulamode_dynamic
```

`fritz settings mesh` shows whether mesh devices choose the best connection
themselves (`backhaul_steering`, "Mesh devices choose the best connection
automatically" in the web interface) and the role of the device in the mesh.

### Calls, phonebook and event log

These name people and numbers. The CLI shows them to you; `fritz mcp` only
serves them with `--personal`.

```console
$ fritz calls --limit 3
DATE              TYPE      NUMBER        NAME   OWN NUMBER  DEVICE   MINUTES
2026-11-20 17:40  missed    015712345678         03012345             0
2026-11-20 09:15  outgoing  03098765      Alice  03012345    Kitchen  12
2026-11-19 20:01  incoming  03098765      Alice  03012345    Kitchen  65

$ fritz phonebook alice
PHONEBOOK  NAME   NUMBER        TYPE
Phonebook  Alice  03098765      home
Phonebook  Alice  015798765432  mobile

$ fritz log --limit 2
TIME                 MESSAGE
2026-11-20 17:40:12  WLAN device logged on (5 GHz), 866 Mbit/s, laptop, IP 192.168.178.20
2026-11-20 08:02:55  Internet connection established successfully.
```

### Any TR-064 action

`fritz tr064` reaches every service and action of the device. Listing and
describing them needs no login. `call` only runs actions that read and
refuses all others before anything is sent:

```console
$ fritz tr064 actions WLANConfiguration1
write     SetEnable
read      GetInfo
critical  SetSecurityKeys
secret    GetSecurityKeys
…

$ fritz tr064 describe WLANConfiguration1 GetInfo
GetInfo (read)
  out  NewEnable    boolean
  out  NewStatus    string (Up, Disabled)
  out  NewChannel   ui1
  out  NewSSID      string
  …

$ fritz tr064 call WLANConfiguration1 GetInfo
NewEnable   1
NewStatus   Up
NewChannel  6
NewSSID     Home
…

$ fritz tr064 call WLANConfiguration1 GetSecurityKeys
Error: GetSecurityKeys is secret: the name points to a secret; only actions that read are called
```

Inputs go after the action, like `fritz tr064 call Hosts1 GetGenericHostEntry NewIndex=0`.

### JSON and YAML output

The commands from `overview` to `log` print a table on a terminal and JSON
otherwise, for scripts and agents, and so do `config list`, `cache show` and
`version`. `--output` (`-o`) picks `text`, `json` or `yaml`. The other
`config` and `cache` commands, `tls`,
`auth` and `tr064` always print text.

To keep text in pipes, like `fritz hosts | grep phone`, set
`FRITZ_OUTPUT=text` in your shell; `--output` still wins over it.

```console
$ fritz wlan --output json | jq '.[0]'
{
  "service": "WLANConfiguration1",
  "ssid": "Home",
  "band": "2.4 GHz",
  "channel": 6,
  "standard": "be",
  "enabled": true,
  "status": "Up",
  "guest": false,
  "client_count": 2
}
```

YAML has the same fields as JSON. Strings from the device are quoted where
needed, so a host name like `a: b` stays a string. With `--output yaml`,
errors are YAML on stderr as well, with the fields of the errors of the MCP
server: `error`, and for faults of the device `status` and `retry_after`.

```console
$ fritz capabilities --output yaml
features:
- dsl
- wlan
rights:
- area: BoxAdmin
  access: readwrite
two_factor: true
services: 37
```

### Several devices and the environment

Each device gets a profile; `--profile` picks one, the first is the default:

```sh
fritz --profile repeater config init --host 192.168.178.4
fritz --profile repeater tls trust
fritz --profile repeater auth login
fritz --profile repeater overview
```

`fritz config list` shows all profiles and marks the default one:

```console
$ fritz config list
DEFAULT  PROFILE   HOST           USER   TRUSTED
*        default   192.168.178.1  agent  yes
         repeater  192.168.178.4  -      yes
```

In scripts and containers, the environment replaces the profile and the
keychain: `FRITZ_HOST`, `FRITZ_USERNAME`, `FRITZ_PASSWORD` (or
`FRITZ_PASSWORD_COMMAND`) and `FRITZ_TLS_FINGERPRINT`. [docs/box-setup.md](https://github.com/udondan/fritz/blob/main/docs/box-setup.md#5-without-a-configuration-file)
lists all variables.

### Debug log

`-v` logs one line per request to stderr: method, action, URL, status and
duration. `-vv` adds the details: the profile and where the password came
from, the Digest login (without nonces or hashes), the check of the
certificate and faults that ask to wait. stdout stays the same, so `fritz
mcp` can log too.

```console
$ fritz -v wlan
2026-11-23T12:00:00.000000Z  INFO tr064::client: request method=POST url=https://192.168.178.1:49443/upnp/control/wlanconfig1 action="urn:dslforum-org:service:WLANConfiguration:1#GetInfo" status=200 elapsed_ms=41
```

`FRITZ_LOG` sets the level instead: `info`, `debug` or `trace`, which adds
the arguments and answers of every call. It also takes a level per crate,
like `tr064=trace,fritz_core=debug`. `FRITZ_LOG_FILE` appends the log to a
file as well, readable only by you, for example for `fritz mcp`, whose
client may not show stderr.

The log hides the same secrets as the output: passwords, keys and session
IDs become `***`, even in URLs. Calls, contacts and the event log only
appear with their size, and documents like host lists never appear at all.

### Cache

Before it calls an action, `fritz` needs the service description (SCPD) of
its service. These only change with the firmware, so `fritz` keeps them in
the cache directory: `~/.cache/fritz` on Linux (or below `$XDG_CACHE_HOME`),
`~/Library/Caches/fritz` on macOS, and below `%LOCALAPPDATA%` on Windows.
`FRITZ_CACHE_DIR` sets another one.

Every run still fetches the device description, and uses the cache only if
it names the same device, model and firmware version; after a firmware
update the cache of the device is replaced. Devices whose description has no
firmware version, like some FRITZ!Repeaters, are not cached. `--no-cache` (or
`FRITZ_NO_CACHE=1`) skips the cache.

```console
$ fritz cache show
MODEL           FIRMWARE   DEVICE                                     SCPDS  SIZE
FRITZ!Box 7690  285.08.25  uuid:00000000-0000-4000-8000-000000000001  12     98 KiB
in /home/me/.cache/fritz

$ fritz cache clear
removed the descriptions of 1 device from /home/me/.cache/fritz
```

### Shell completion

`fritz completion <shell>` prints the completion script for bash, zsh, fish,
elvish or PowerShell. It completes the commands, the options and the names
of the profiles from the configuration file. On every completion the script
calls `fritz` from the `PATH`, which never contacts the device. Write the
script again after an update of fritz:

```sh
fritz completion bash > ~/.local/share/bash-completion/completions/fritz
fritz completion zsh > "${fpath[1]}/_fritz"
fritz completion fish > ~/.config/fish/completions/fritz.fish
```

`fritz completion --help` shows the commands for elvish and PowerShell.

### Version and updates

`fritz version` shows the version, the commit, the build date, the target
and whether the build has the feature `write`. Add `--output json` or
`--output yaml` for bug reports and scripts. `fritz --version` prints only the version.

```console
$ fritz version
Version   0.1.1
Commit    0123456789ab
Built     2026-11-23
Target    aarch64-apple-darwin
Features  none
```

`fritz version --check` asks GitHub for the latest release of `fritz`,
ignoring pre-releases, and shows whether it is newer, with its URL and the
first lines of its release notes. The exit code tells scripts the result:

| Exit code | Meaning |
|---|---|
| 0 | This is the latest version. |
| 10 | A newer version is available. |
| 1 | The check failed, for example offline. |

`fritz` checks only when asked to, never on its own and never in `fritz mcp`,
and it downloads and installs nothing. The request goes to `api.github.com`
over HTTPS with the root certificates of the system and the proxy in
`HTTPS_PROXY`, and sends nothing but a generic user agent.

## MCP server

`fritz mcp` serves the state of the device to an MCP client on stdin and
stdout. It only reads. Store the password first with `fritz auth login`, then
add the server to the client, for example to Claude Code:

```sh
claude mcp add fritz -- fritz --host 192.168.178.1 --user agent mcp
```

| Tool                    | Returns                                                     |
| ----------------------- | ----------------------------------------------------------- |
| `fritz_overview`        | Model, firmware, uptime, connection, rights, `system` load  |
| `fritz_internet`        | Internet connection, DNS, rates, DSL line, `dsl_details`    |
| `fritz_firmware`        | Installed and offered firmware, auto update, last update    |
| `fritz_wlan`            | Wireless networks, guest network, `clients`, `details`      |
| `fritz_interfaces`      | LAN and WLAN counters of bytes, packets, errors; LAN ports  |
| `fritz_hosts`           | Known hosts, paginated, or one host by `mac` or `ip`        |
| `fritz_mesh`            | Mesh nodes with IPs and links, or a tree of who is where    |
| `fritz_powerline`       | Powerline adapters, whether active, updates                 |
| `fritz_smarthome`       | Smart Home devices (FRITZ!OS 8.20 and later)                |
| `fritz_ports`           | Port forwardings and whether UPnP is on                     |
| `fritz_telephony`       | Numbers, accounts, deflections, answering machines, DECT    |
| `fritz_capabilities`    | Features of the device, rights of the user, two-factor      |
| `fritz_users`           | Users (names need `--personal`), login settings, hints      |
| `fritz_settings`        | Settings like DNS, IPv6, MTU, DHCP and time, secrets hidden |
| `fritz_list_services`   | TR-064 services, or the actions of one with their class     |
| `fritz_describe_action` | The arguments of an action and their types                  |
| `fritz_call_action`     | Calls an action that only reads, with secrets hidden        |

Calls, contacts and the event log are personal, so `fritz mcp` only serves
them with `--personal` (or `FRITZ_MCP_PERSONAL=true`). Without it,
`fritz_call_action` refuses the actions behind them and hides the event log
that `DeviceInfo:GetInfo` returns as well. The option adds three tools:

| Tool              | Returns                                                     |
| ----------------- | ----------------------------------------------------------- |
| `fritz_calls`     | Latest calls, filtered by `kind`                            |
| `fritz_phonebook` | Contacts of all phonebooks, filtered by `search`, paginated |
| `fritz_log`       | Event log, filtered by `search`                             |

Errors from the device come back as JSON with a `status` (`denied`,
`confirmation_required`, `blocked` or `busy`) and, where it helps,
`retry_after` in seconds.

Agents that use the CLI instead can load the skill in
[skills/fritz/SKILL.md](https://github.com/udondan/fritz/blob/main/skills/fritz/SKILL.md).

### Docker

The image runs `fritz`; without a command it starts the MCP server. A
container has no keychain, so the device and the password come from the
environment. Get the fingerprint once with `fritz tls fingerprint`.

```sh
docker run --rm -i \
  -e FRITZ_HOST=192.168.178.1 -e FRITZ_USERNAME=agent \
  -e FRITZ_PASSWORD -e FRITZ_TLS_FINGERPRINT \
  udondan/fritz
```

`mise run docker-build` builds the image from source. For the Docker MCP
Toolkit, `mise run docker` builds it and adds
[docker-mcp.yml](https://github.com/udondan/fritz/blob/main/docker-mcp.yml) to the profile `fritz`. Set the secrets
`fritz.host`, `fritz.username`, `fritz.password` and `fritz.fingerprint`
with `docker mcp secret set`. The container must be able to reach the box.

## Changes

Release builds only read. Builds with the feature `write` add `fritz change`,
which plans a change, shows the current and the new values, and applies it
with the token of the plan:

```sh
cargo build -p fritz --features write
fritz change identity        # add it to [write] allow in the configuration
FRITZ_ALLOW_WRITE=1 fritz change plan WLANConfiguration3 SetEnable NewEnable=0
FRITZ_ALLOW_WRITE=1 fritz change apply WLANConfiguration3 SetEnable NewEnable=0 --token <token>
```

A plan shows only the inputs of the action, but a device may change other
settings along with them. `apply` reads the service before and after the
change and lists the other values that changed under `also changed`.

`fritz reboot` restarts a device the same way. A restart is critical, so it
also needs `FRITZ_ALLOW_CRITICAL=1`. The plan names the device and warns
that internet, WLAN and telephony are down while it restarts. `--wait`
waits until the device is back. For example, a repeater with its own
profile:

```sh
export FRITZ_ALLOW_WRITE=1 FRITZ_ALLOW_CRITICAL=1
fritz --profile repeater reboot                        # plan: device, uptime, warning, token
fritz --profile repeater reboot --token <token> --wait # restart and wait until it is back
```

While it waits, `fritz reboot` only reads `DeviceInfo:GetInfo`, every 5
seconds (`--interval`) for at most 5 minutes (`--budget`, at most `1h`).
The device is `back` once it answers with a lower uptime than before and
the same serial number. The other outcomes are `accepted` (without
`--wait`), `timeout` and `unknown`, when the connection broke while the
restart was sent. Then the device may or may not restart. `fritz reboot`
never sends the restart again on its own, and exits with an error after
`timeout` and `unknown`.

With `FRITZ_ALLOW_WRITE=1`, `fritz mcp` of such a build also offers
`fritz_plan_change` and `fritz_apply_change`, which work the same way.

Changes are only made to devices on the allow list, and never to those on the
deny list. See [SECURITY.md](https://github.com/udondan/fritz/blob/main/SECURITY.md#changes) for all checks.

## Crates

| Crate | crates.io | docs.rs | Purpose |
| --- | --- | --- | --- |
| [`tr064`](https://github.com/udondan/fritz/tree/main/crates/tr064) | [![tr064 on crates.io][tr064-crate-badge]][tr064-crate] | [![tr064 on docs.rs][tr064-docs-badge]][tr064-docs] | Vendor-neutral TR-064 (UPnP/SOAP) client |
| [`fritz-core`](https://github.com/udondan/fritz/tree/main/crates/fritz-core) | [![fritz-core on crates.io][fritz-core-crate-badge]][fritz-core-crate] | [![fritz-core on docs.rs][fritz-core-docs-badge]][fritz-core-docs] | FRITZ!-specific logic: login, Smart Home API, policy, redaction |
| [`fritz-sim`](https://github.com/udondan/fritz/tree/main/crates/fritz-sim) | [![fritz-sim on crates.io][fritz-sim-crate-badge]][fritz-sim-crate] | [![fritz-sim on docs.rs][fritz-sim-docs-badge]][fritz-sim-docs] | Fake FRITZ!Box for tests, built from recorded fixtures |
| [`fritz`](https://github.com/udondan/fritz/tree/main/crates/fritz) | [![fritz on crates.io][fritz-crate-badge]][fritz-crate] | – | The binary: CLI and `fritz mcp` |

[tr064-crate-badge]: https://img.shields.io/crates/v/tr064?label=
[tr064-crate]: https://crates.io/crates/tr064
[tr064-docs-badge]: https://img.shields.io/docsrs/tr064?label=
[tr064-docs]: https://docs.rs/tr064
[fritz-core-crate-badge]: https://img.shields.io/crates/v/fritz-core?label=
[fritz-core-crate]: https://crates.io/crates/fritz-core
[fritz-core-docs-badge]: https://img.shields.io/docsrs/fritz-core?label=
[fritz-core-docs]: https://docs.rs/fritz-core
[fritz-sim-crate-badge]: https://img.shields.io/crates/v/fritz-sim?label=
[fritz-sim-crate]: https://crates.io/crates/fritz-sim
[fritz-sim-docs-badge]: https://img.shields.io/docsrs/fritz-sim?label=
[fritz-sim-docs]: https://docs.rs/fritz-sim
[fritz-crate-badge]: https://img.shields.io/crates/v/fritz?label=
[fritz-crate]: https://crates.io/crates/fritz

## Security

`fritz` is read-only by default. See [SECURITY.md](https://github.com/udondan/fritz/blob/main/SECURITY.md) for the security
model and how to report a vulnerability.

## Contributing

See [CONTRIBUTING.md](https://github.com/udondan/fritz/blob/main/CONTRIBUTING.md).

## License

[Apache-2.0](https://github.com/udondan/fritz/blob/main/LICENSE)

## Disclaimer

This is an unofficial, independent open source project. It is not affiliated
with FRITZ! GmbH (formerly AVM). FRITZ! and FRITZ!Box are trademarks of
FRITZ! GmbH. The names are only used to describe compatibility. Use this tool
at your own risk. Changing router settings can cut off your network access.
