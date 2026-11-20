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

## 4. Note the address of your box

`fritz` needs the address of your box, for example `192.168.178.1`. Use the IP
address rather than `fritz.box`: newer FRITZ!OS versions may no longer hand
out the name `fritz.box` in every setup.
