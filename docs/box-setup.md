# Setting up your FRITZ!Box for fritz

`fritz` logs in to your FRITZ!Box with its own user account. This guide creates
that account with no more rights than `fritz` needs.

Menu names are those of FRITZ!OS 8.x in German, with an English translation.
They can differ slightly between versions and languages. If they differ on
your box, please open an issue.

## 1. Allow access for applications (TR-064)

`fritz` uses TR-064, the interface the FRITZ!Box offers to applications.

1. Open **Heimnetz → Netzwerk → Netzwerkeinstellungen** (Home Network →
   Network → Network Settings).
2. Under **Heimnetzfreigaben** (Home Network Sharing), turn on **Zugriff für
   Anwendungen zulassen** (Allow access for applications).
3. Click **Übernehmen** (Apply).

## 2. Create a user for fritz

1. Open **System → FRITZ!Box-Benutzer** (System → FRITZ!Box Users) and click
   **Benutzer hinzufügen** (Add User).
2. Name it for example `fritz` and give it a long random password, for example
   from your password manager. Don't reuse a password.
3. Turn **off** **Zugang auch aus dem Internet erlaubt** (Access from the
   internet allowed). `fritz` only runs in your home network.
4. Under **Berechtigungen** (Permissions), turn on only what you want `fritz`
   to read:

   | Permission                                                                                                | Needed for                                    |
   | --------------------------------------------------------------------------------------------------------- | --------------------------------------------- |
   | **FRITZ!Box Einstellungen** (FRITZ!Box settings)                                                          | Almost everything: status, network, WLAN, internet |
   | **Sprachnachrichten, Faxnachrichten, FRITZ!App Fon und Anrufliste** (Voice messages, faxes, FRITZ!App Fon and call list) | Call list and phone book (optional)           |
   | **Smart Home**                                                                                            | Smart home devices (optional)                 |

   According to the TR-064 documentation of FRITZ!, the settings permission
   includes the phone permission, so with **FRITZ!Box Einstellungen** on, the
   second row changes nothing for `fritz`.

   Leave everything else off: NAS contents, VPN and apps aren't needed.
5. Click **Übernehmen** (Apply).

> **The FRITZ!Box has no read-only permission.** **FRITZ!Box Einstellungen**
> allows changing settings as well. `fritz` itself makes sure it only reads:
> it can't send any action that changes the box, unless it was built with
> write support and you allowed it explicitly, and release builds don't include
> it. Treat the password of this user like the password of your FRITZ!Box.

## 3. Leave the confirmation of changes on

Under **System → FRITZ!Box-Benutzer → Anmeldung im Heimnetz** (System →
FRITZ!Box Users → Login to the Home Network), the box can ask you to confirm
some changes with a button press or a phone code. Leave this on. `fritz` never
bypasses it: when the box asks for a confirmation, `fritz` reports that and
stops.

## 4. Set up fritz

`fritz` keeps the address of your box, the user and the trusted certificate
in a profile, in `~/.config/fritz/config.toml` on Linux and macOS (or below
`$XDG_CONFIG_HOME`) and in `%APPDATA%\fritz\config` on Windows. Use the IP address rather than
`fritz.box`: newer FRITZ!OS versions may no longer hand out the name
`fritz.box` in every setup.

```sh
fritz config init --host 192.168.178.1 --user agent
```

`fritz` talks to the box over HTTPS on port 49443. The box uses a self-signed
certificate, so `fritz` asks you once whether to trust it and then refuses
any other certificate:

```sh
fritz tls trust
```

Compare the fingerprint it shows with the certificate in the web interface of
the box, to make sure nobody else answered. When you replace the certificate
of the box, run `fritz tls trust` again.

Store the password in the keychain of your system: the Keychain on macOS,
the Credential Manager on Windows, or the Secret Service on Linux:

```sh
fritz auth login
```

`fritz config show` shows the settings in effect, and `fritz auth status`
where the password comes from.

### Devices with only a password

FRITZ!Repeaters, and a FRITZ!Box set to log in with the password alone, have
no users. Leave out `--user` for them; the keychain then keeps the password
under the host alone:

```sh
fritz --profile repeater config init --host 192.168.178.4
fritz --profile repeater tls trust
fritz --profile repeater auth login
fritz --profile repeater overview
```

These devices take any user name over TR-064 and use a user of their own
instead, so `fritz` sends the historic default `dslf-config`. On a FRITZ!Box,
a user of its own with only the rights it needs is the safer choice.

## 5. Without a configuration file

In containers and scripts, the environment can replace the profile and the
keychain:

| Variable | Option | Meaning |
|---|---|---|
| `FRITZ_HOST` | `--host` | `192.168.178.1` means `https://192.168.178.1:49443`. Use `http://192.168.178.1:49000` for unencrypted access. |
| `FRITZ_USERNAME` | `--user` | The user on the box. Leave it out for devices with only a password. |
| `FRITZ_PASSWORD` | | The password. It takes precedence over the keychain. |
| `FRITZ_TLS_FINGERPRINT` | `--fingerprint` | The SHA-256 fingerprint of the certificate, from `fritz tls fingerprint`. |
| `FRITZ_PROFILE` | `--profile` | The profile to use instead of the default one. |
| `FRITZ_CONFIG` | `--config` | Another configuration file. |
| `FRITZ_NO_KEYCHAIN` | | Any value keeps `fritz` away from the keychain. |
| `FRITZ_NO_CACHE` | `--no-cache` | `1` fetches the service descriptions from the device instead of the cache. |
| `FRITZ_CACHE_DIR` | | Another directory for the cache of the service descriptions. |

Options and variables take precedence over the profile. The user and the
certificate of the profile apply only to its own host (the user also over
another scheme or port): with `--host` set to
another device, like `fritz --host http://192.168.178.4:49000 overview` for a
repeater, `fritz` uses neither.
