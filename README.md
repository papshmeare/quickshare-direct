# quickshare-direct

**Quick Share (Nearby Share) for Linux that doesn't need both devices on the same Wi-Fi.**

Existing Linux implementations only transfer over a shared local network, and newer Pixels (with
the AirDrop-compatible Quick Share) drop off Wi-Fi while looking for devices, so they can't even
see them. Phone-to-phone Quick Share needs no shared network: devices find each other over
Bluetooth and move the data over a direct Wi-Fi link. quickshare-direct does that on Linux:

1. **Discovery + first contact over Bluetooth LE** (works with the phone off Wi-Fi).
2. **Encrypted handshake** (UKEY2, PIN shown on both sides) over that Bluetooth link.
3. **Upgrade to Wi-Fi speed**: the laptop creates a **Wi-Fi Direct group** next to its normal
   Wi-Fi connection (which keeps working) and hands the credentials to the phone; or, if the
   phone is on the same Wi-Fi, the transfer just moves to the local network. A plain hotspot is
   the fallback.
4. **Desktop notifications**: Accept/Decline (with the PIN), then "Open folder".

| | Status |
|---|---|
| Receive from Android (tested: Pixel 10, Android 16) | works |
| Phone off Wi-Fi → Wi-Fi Direct (75 MB, ~5 MB/s) | works |
| Phone on the same Wi-Fi → LAN | works |
| Bluetooth Classic first contact | experimental (`QSD_BT_CLASSIC=1`) |
| Send to Android → phone's Wi-Fi Direct (300 MB, ~36 MB/s) | works (`quickshare-direct send`, file manager) |
| Packages: Debian/Ubuntu, Fedora, Arch, generic (x86_64, aarch64) | one-line install, see below |

### Install

Debian 12+ / Ubuntu 22.04+ (and Mint, Pop!_OS, ...), Fedora, Arch (and Manjaro, EndeavourOS, ...),
others via a generic build; x86_64 and aarch64:

```
curl -fsSL https://raw.githubusercontent.com/papshmeare/quickshare-direct/main/install.sh | sh
```

It installs the latest [release](https://github.com/papshmeare/quickshare-direct/releases) with
your package manager (`.deb`, `.rpm`, `.pkg.tar.zst`; dependencies come from your distribution),
opens the Quick Share ports if firewalld or ufw is active, and starts the receiver. Run the same
line again to update; `curl -fsSL .../install.sh | sh -s -- --uninstall` removes it. Prefer to
see what happens first? Download `install.sh` and read it, or install the package from the
release page by hand (`sudo apt install ./quickshare-direct_amd64.deb`,
`sudo dnf install ./quickshare-direct.x86_64.rpm`,
`sudo pacman -U quickshare-direct-x86_64.pkg.tar.zst`).

What gets installed:
- `quickshare-direct`: the receiver (a user service in every graphical session, with
  notifications) and the `send` / `devices` commands;
- two small root helpers, started on demand through systemd (`quickshare-ap@USER` creates the
  direct Wi-Fi link when receiving, `quickshare-join@USER` joins the phone's when sending), and a
  polkit rule that lets you start only your own instances;
- a NetworkManager drop-in that leaves the temporary link interfaces alone, a unit that keeps
  Bluetooth connectable, firewalld/ufw service definitions, and file manager entries.

Needs: systemd, BlueZ, NetworkManager with wpa_supplicant (the default on these distributions;
not iwd), and a Wi-Fi card that can run a second interface next to the normal connection (most
Intel/MediaTek/Qualcomm cards). Notifications need a daemon that shows action buttons (GNOME,
KDE, swaync, mako, ...). Ubuntu 22.04's older polkit can't let users start the helpers, so there
only Bluetooth and same-network transfers work. Set your Wi-Fi regulatory domain
(`sudo iw reg set XX`): with the default world domain Linux won't start a group owner on 5 GHz.
Settings: `~/.config/quickshare-direct/env` (`QSD_NAME=...`, `QSD_DIR=...`) for the receiver,
`/etc/default/quickshare-direct` for the helpers. Many Wi-Fi chips can't host the direct link on a
second channel next to your normal connection, which matters when your router uses a channel a
laptop may not host on (radar/DFS channels such as 100): then the laptop leaves your Wi-Fi network
for the few seconds of the transfer and reconnects afterwards (`QS_SINGLE_CHANNEL=auto`; `always`
for full speed when sending too, `never` to keep the connection and fall back to Bluetooth).

From source: `make && sudo make install` (needs cargo ≥ 1.85, protoc, libdbus headers), then
`systemctl --user enable --now quickshare-direct`.

### NixOS

```nix
# flake.nix inputs
quickshare-direct = { url = "github:papshmeare/quickshare-direct"; inputs.nixpkgs.follows = "nixpkgs"; };

# a module
{ inputs, ... }: {
  imports = [ inputs.quickshare-direct.nixosModules.default ];
  services.quickshare-direct = {
    enable = true;
    user = "alice";               # whose desktop session runs it
    wifiInterface = "wlp2s0";     # ip -br link
  };
}
```
This sets up the receiver (user service in the graphical session), the root helper that creates
the direct Wi-Fi link on demand (`quickshare-ap.service`, startable by that user via polkit),
the helper that joins a phone's Wi-Fi Direct group when sending (`quickshare-join.service`),
firewall ports, Bluetooth "connectable", and keeps NetworkManager off the link interfaces.
Set your Wi-Fi regulatory domain (`iw reg set XX`): with the default world domain Linux won't
start an access point / group owner on 5 GHz.

### Sending

On the phone, open Quick Share and tap **Receive** (or make it visible to everyone), then:

```
quickshare-direct send photo.jpg notes.pdf      # to the one phone in receive mode nearby
quickshare-direct send --to Pixel video.mp4     # pick by name when several are around
quickshare-direct devices                       # list phones in receive mode
```
It finds the phone over Bluetooth, shows the PIN, and once you accept on the phone the files go
over the phone's Wi-Fi Direct group (joined on a second interface, your Wi-Fi stays connected).
In the file manager: Thunar's **Send To → Phone (Quick Share)** (any file), or Open With →
"Send with Quick Share" (common file types); it reports through notifications.

### How it works / research notes

[docs/DEV_NOTES.md](docs/DEV_NOTES.md) (findings, phone logs, protocol details) and
[docs/BLE_RECEIVER_DISCOVERY.md](docs/BLE_RECEIVER_DISCOVERY.md) (the BLE receiver path).

Built on [rQuickShare](https://github.com/Martichou/rquickshare) by Martichou and contributors,
with Bluetooth work by [nozwock](https://github.com/nozwock) and
[martinalderson](https://github.com/martinalderson). Protocol reference: Google's
[Nearby](https://github.com/google/nearby) library (Apache-2.0). License: GPL-3.0, like rQuickShare.

Releases: CI (`.github/workflows/packages.yml`) builds and install-tests the packages on every
push; bumping `VERSION` on `main` publishes the release `qsd-v<VERSION>`.

Repository layout: quickshare-direct is `core_lib` (library + the `quickshare-direct` binary) and
`packaging/`; that's all the packages, the NixOS module and CI build. `app/` is rQuickShare's
Tauri desktop app, kept from the fork but not built, shipped or tested here: its sending only
works on a shared Wi-Fi (mDNS + TCP), without the Bluetooth / Wi-Fi Direct path of
`quickshare-direct send`. The same goes for the other upstream leftovers (`snap/`, `BUILD.md`,
`CHANGELOG.md`, release-please, the `build`/`lint` workflows, which run on upstream's `master`).
The rest of this README below is rQuickShare's and describes that app.

Development: `nix develop`, then in `core_lib`: `cargo run --bin quickshare-direct`
(env: `QSD_DIR`, `QSD_NAME`, `QSD_PORT`, `QSD_BWU_PORT`, `QSD_BWU=lan|hotspot`, `RUST_LOG`).

---

*Original rQuickShare README below.*

<div align="center">
  <h1>rquickshare</h1>

  <p>
    <strong>NearbyShare/QuickShare for Linux and MacOS</strong>
  </p>
  <p>

[![CI](https://github.com/Martichou/rquickshare/actions/workflows/build.yml/badge.svg)](https://github.com/Martichou/rquickshare/actions)
[![CI](https://github.com/Martichou/rquickshare/actions/workflows/lint.yml/badge.svg)](https://github.com/Martichou/rquickshare/actions)

  </p>
</div>

![demo image](.github/demo.png)

Installation
--------------------------

You simply have to download the latest release.

**Important notes:**
- The minimum GLIBC version supported is included in the pkg name.
  - You can check yours with `ldd --version`.
- RQuickShare is distributed with two version (main & legacy):
  - Legacy is for compatibility with older Ubuntu versions.
  - Main is for future support of newer versions of Ubuntu.

#### macOS

Simply install the .dmg.

Note that you may have to first allow the app to install under `Settings > Privacy & Security > Security` (you should see a dialog asking for permission)

#### Linux

##### Install dependencies

RQuickShare requires one of the following libraries to be installed:

- `libayatana-appindicator`
- `libappindicator3`

The files should (in theory) install those dependencies by themselves, but if this is not the case you may have to install those manually.

##### Install rquickshare
```bash
sudo dpkg -i r-quick-share_${VERSION}.deb
```

#### Debian
```bash
sudo dpkg -i r-quick-share_${VERSION}.deb
```

#### RPM
```bash
sudo rpm -i r-quick-share-${VERSION}.rpm
```

#### DNF (preferred over RPM)
```bash
sudo dnf install r-quick-share-${VERSION}.rpm
```

#### AppImage (no root required)

AppImage is a little different. There's no installation needed, you simply have to give it the executable permission (+x on a chmod) to run it.

```bash
chmod +x r-quick-share_${VERSION}.AppImage
```

You can then either double click on it, or run it from the cmd line:

```bash
./r-quick-share_${VERSION}.AppImage
```

#### Snap

The snap is not yet on the store, but you can install it with the following (you may need sudo):

```bash
snap install --dangerous r-quick-share_${VERSION}.snap
```

---

<details>
<summary>Unofficial Installation Methods</summary>

#### AUR (Arch)

For Arch Linux, you can install it from the AUR by using an AUR helper like yay:

```bash
yay -S r-quick-share
```

### Nix

Available here: [NixOS](https://search.nixos.org/packages?channel=24.05&show=rquickshare&from=0&size=50&sort=relevance&type=packages&query=rquickshare)

A nix-shell will temporarily modify your $PATH environment variable. This can be used to try a piece of software before deciding to permanently install it.

```bash
$ nix-shell -p rquickshare
```
</details>

---

Limitations
--------------------------

- **Wi-Fi LAN only**. Your devices need to be on the same network for this app to work.

FAQ
--------------------------

### My Android device doesn't see my laptop

Make sure both your devices are on the same WiFi network. mDNS communication should be allowed on the network; this may not be the case if you're on a public network (coffee shops, airports, etc.).

### My laptop doesn't see my Android device

For some reason, Android doesn't broadcast its mDNS service all the time, even when in "Everyone" mode.

The first solution (implemented in RQuickShare for Linux) is to broadcast a bluetooth advertisement so that Android will then make its mDNS available.
Of course, for this you need to have bluetooth on your laptop/desktop. If you don't have that, continue reading.

As a workaround, you can use the "[Files](https://play.google.com/store/apps/details?id=com.google.android.apps.nbu.files)" app on your Android device and go to the "Nearby Share" tab (if it's not present, continue reading).

A second workaround is to download a Shortcut maker (see [here](https://xdaforums.com/t/how-to-manually-create-a-homescreen-shortcut-to-a-known-unique-android-activity.4336833)) to create a shortcut to the particular intent:

- Method A:
	- Activity: `com.google.android.gms.nearby.sharing.ReceiveSurfaceActivity`

- Method B:
	- Action: `com.google.android.gms.RECEIVE_NEARBY`
	- Mime type: `*/*`

_Note: Samsung did something shady with Quick Share, so the above workaround may not work. Unfortunately, there's no alternative at the moment. Sorry._

### When sharing a file, my phone appears and disappears "randomly"

TLDR: This is normal if you're just using bluetooth (as explained in the previous point).

Android will see that your laptop/desktop is trying to share a file and will reveal itself. But for some reason, Android will de-register its service from time to time and will only then be revealed again once it detects the bluetooth message again.

### Once I close the app, it won't reopen

Make sure the app is really closed by running:

```bash
ps aux | grep r-quick-share
```

If you see that the process is still running, it's because the app is not closed. This may be an intended behavior: when closing the window, the app won't stop and instead is still running and accessible via the system tray icon. However, if your distribution doesn't support/hasn't enabled this, it may be an issue for you.

If you want to **really** close the app when clicking on the close button, you can change that inside the app by clicking on the three dots and then "Stop app on close".

### My firewall is blocking the connection

In this case, you may want to configure a static port to allow it in your firewall. You can do so by modifying the config file as follow:

```bash
# linux
vim ./.local/share/dev.mandre.rquickshare/.settings.json

# mac
vim Library/Application\ Support/dev.mandre.rquickshare/.settings.json

# to be sure
find $HOME -name ".settings.json"
```

> [!WARNING]
>
> The json must stay valid after your modification; for example, if "port" is the last item of the JSON it must not have a comma after it, otherwise the config will be reset.

```json
{
	...existing_config...,
	"port": 12345
}
```

By default the port is random (the OS will decide).

### The app opens but I just get a blank window or cannot run it.

This happens for some users running Linux + NVIDIA cards.

The workaround is to start RQuickShare with an env variable defined as follows:

```bash
env WEBKIT_DISABLE_COMPOSITING_MODE=1 rquickshare
```

Alternatively, you may try the `legacy` variant.

WIP Notes
--------------------------

`rquickshare` is still in development (WIP) and currently only supports Linux even though it should be compatible with macOS too. Keep in mind that the design may change between versions, so flexibility is key.

Got feedback or suggestions? We'd love to hear them! Feel free to open an issue and share your thoughts.

Credits
--------------------------

This project wouldn't exist without those amazing open-source project:

- https://github.com/grishka/NearDrop
- https://github.com/vicr123/QNearbyShare


Contributing
--------------------------

Pull requests are welcome. For major changes, please open an issue first to discuss what you would like to change.
