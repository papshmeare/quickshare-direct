{
  description = "quickshare-direct: Quick Share (Nearby Share) for Linux over Bluetooth + Wi-Fi Direct, no shared Wi-Fi needed";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in
    {
      packages = forAll (pkgs: rec {
        default = quickshare-direct;
        quickshare-direct = pkgs.rustPlatform.buildRustPackage {
          pname = "quickshare-direct";
          version = nixpkgs.lib.removeSuffix "\n" (builtins.readFile ./VERSION);
          src = ./core_lib;
          cargoLock = {
            lockFile = ./core_lib/Cargo.lock;
            allowBuiltinFetchGit = true;   # mdns-sd and sys_metrics come from git
          };
          cargoBuildFlags = [ "--bin" "quickshare-direct" ];
          nativeBuildInputs = with pkgs; [ pkg-config protobuf ];
          buildInputs = with pkgs; [ dbus ];
          doCheck = false;
          postInstall = ''
            install -Dm755 ${./packaging/linux/quickshare-ap} $out/libexec/quickshare-ap
            install -Dm755 ${./packaging/linux/quickshare-join} $out/libexec/quickshare-join
            install -Dm755 ${./packaging/linux/quickshare-bt-setup} $out/libexec/quickshare-bt-setup
            patchShebangs $out/libexec
            # "Send with Quick Share" in the file manager's Open With menu.
            install -Dm644 ${./packaging/linux/quickshare-direct-send.desktop} \
              $out/share/applications/quickshare-direct-send.desktop
            # ... and in Thunar's Send To menu, for any file.
            install -Dm644 ${./packaging/linux/quickshare-direct-sendto.desktop} \
              $out/share/Thunar/sendto/quickshare-direct.desktop
            substituteInPlace $out/share/applications/quickshare-direct-send.desktop \
              $out/share/Thunar/sendto/quickshare-direct.desktop \
              --replace-fail "Exec=quickshare-direct" "Exec=$out/bin/quickshare-direct"
          '';
          meta = {
            description = "Quick Share receiver for Linux: Bluetooth first contact, Wi-Fi Direct/hotspot/LAN transfer";
            homepage = "https://github.com/papshmeare/quickshare-direct";
            license = pkgs.lib.licenses.gpl3Only;
            mainProgram = "quickshare-direct";
            platforms = pkgs.lib.platforms.linux;
          };
        };
      });

      # NixOS module: `services.quickshare-direct.enable = true; ...user = "me";`
      nixosModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.quickshare-direct;
          pkg = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        in
        {
          options.services.quickshare-direct = {
            enable = lib.mkEnableOption "the quickshare-direct Quick Share receiver";
            user = lib.mkOption {
              type = lib.types.str;
              description = "User whose desktop session runs the receiver (may start the Wi-Fi helper).";
            };
            wifiInterface = lib.mkOption {
              type = lib.types.str;
              example = "wlp4s0";
              description = "Wi-Fi station interface; the direct link runs next to it, on its channel.";
            };
            mode = lib.mkOption {
              type = lib.types.enum [ "p2p" "ap" ];
              default = "p2p";
              description = "Direct link: Wi-Fi Direct group (p2p, recommended) or a plain hotspot (ap).";
            };
            port = lib.mkOption { type = lib.types.port; default = 46257; description = "TCP port for incoming transfers."; };
            upgradePort = lib.mkOption { type = lib.types.port; default = 46258; description = "TCP port for the Wi-Fi upgrade."; };
            name = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              description = "Name shown on phones (default: hostname).";
            };
            downloadDir = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              description = "Download folder (default: the user's XDG Downloads).";
            };
            singleChannel = lib.mkOption {
              type = lib.types.enum [ "auto" "always" "never" ];
              default = "auto";
              description = ''
                When the direct Wi-Fi link would need a second channel next to the normal Wi-Fi
                connection (many chips can't host one there, others are slow): auto = leave the
                Wi-Fi network for the transfer only when there's no other way (internet pauses for
                a few seconds), always = whenever the channels differ (full speed), never = don't.
              '';
            };
            leConnInterval = lib.mkOption {
              type = lib.types.str;
              default = "6 12";
              description = ''
                LE connection interval ("MIN MAX" in 1.25 ms units) for the Bluetooth links the laptop
                opens; the kernel default (24 40 = 30-50 ms) limits sending over Bluetooth to ~5 KB/s.
                "off" keeps the kernel default.
              '';
            };
            classicBluetooth = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = "Offer Bluetooth Classic (RFCOMM) first contact (experimental).";
            };
          };

          config = lib.mkIf cfg.enable {
            # `quickshare-direct send FILE...` and the file manager's "Send with Quick Share".
            environment.systemPackages = [ pkg ];
            environment.pathsToLink = [ "/share/Thunar" ];   # Thunar's Send To entries
            # Root helper: creates/removes the direct Wi-Fi link on demand.
            systemd.services.quickshare-ap = {
              description = "Temporary direct Wi-Fi link for Quick Share transfers";
              path = with pkgs; [ iw iproute2 hostapd dnsmasq gawk gnused gnugrep coreutils systemd nftables networkmanager procps ];
              environment = { QS_STA = cfg.wifiInterface; QS_AP = "ap0"; QS_GROUP = "users"; QS_MODE = cfg.mode; QS_SINGLE_CHANNEL = cfg.singleChannel; };
              serviceConfig = {
                Type = "simple";
                ExecStart = "${pkgs.bash}/bin/bash ${pkg}/libexec/quickshare-ap";
                ExecStopPost = "-${pkgs.iw}/bin/iw dev ap0 del";
                RuntimeDirectory = "quickshare";
                RuntimeDirectoryMode = "0750";
              };
            };
            # Root helper for sending: joins the phone's Wi-Fi Direct group / hotspot on a second
            # station interface (qsc0) with the credentials the sender writes to its runtime dir.
            systemd.services.quickshare-join = {
              description = "Join a phone's Wi-Fi Direct group for a Quick Share transfer";
              path = with pkgs; [ iw iproute2 busybox gawk gnused coreutils systemd networkmanager procps ];
              environment = { QS_STA = cfg.wifiInterface; QS_CLI = "qsc0"; QS_GROUP = "users"; QS_USER = cfg.user; QS_SINGLE_CHANNEL = cfg.singleChannel; };
              serviceConfig = {
                Type = "simple";
                ExecStart = "${pkgs.bash}/bin/bash ${pkg}/libexec/quickshare-join";
                ExecStopPost = "-${pkgs.iw}/bin/iw dev qsc0 del";
                RuntimeDirectory = "quickshare-join";
                RuntimeDirectoryMode = "0750";
              };
            };
            security.polkit.extraConfig = ''
              polkit.addRule(function (action, subject) {
                if (action.id == "org.freedesktop.systemd1.manage-units" &&
                    (action.lookup("unit") == "quickshare-ap.service" ||
                     action.lookup("unit") == "quickshare-join.service") &&
                    subject.user == "${cfg.user}") {
                  return polkit.Result.YES;
                }
              });
            '';
            # Keep NetworkManager off the link interfaces (its scans delay an AP by seconds) and stop
            # NixOS restarting wpa_supplicant when they appear (that drops the station connection).
            networking.networkmanager.unmanaged = [ "interface-name:ap0" "interface-name:qsc0" "interface-name:p2p-${cfg.wifiInterface}-*" ];
            services.udev.extraRules = lib.mkAfter ''
              ACTION=="add|remove", SUBSYSTEM=="net", KERNEL=="ap0", RUN="${pkgs.coreutils}/bin/true"
              ACTION=="add|remove", SUBSYSTEM=="net", KERNEL=="qsc0", RUN="${pkgs.coreutils}/bin/true"
              ACTION=="add|remove", SUBSYSTEM=="net", KERNEL=="p2p-${cfg.wifiInterface}-*", RUN="${pkgs.coreutils}/bin/true"
            '';
            networking.firewall.allowedTCPPorts = [ cfg.port cfg.upgradePort ];
            networking.firewall.trustedInterfaces = [ "ap0" ];   # p2p links: the helper adds a rule
            hardware.wirelessRegulatoryDatabase = true;

            # Bluetooth must accept incoming connections (BlueZ keeps it off unless discoverable).
            hardware.bluetooth.enable = lib.mkDefault true;
            systemd.services.quickshare-bt-connectable = {
              description = "Keep the Bluetooth adapter connectable (Quick Share)";
              after = [ "bluetooth.service" ];
              wants = [ "bluetooth.service" ];
              wantedBy = [ "bluetooth.target" ];
              path = with pkgs; [ bluez coreutils ];
              environment.QS_LE_CONN_INTERVAL = cfg.leConnInterval;
              serviceConfig = {
                Type = "oneshot";
                ExecStartPre = "${pkgs.coreutils}/bin/sleep 2";
                ExecStart = "${pkgs.bash}/bin/sh ${pkg}/libexec/quickshare-bt-setup";
              };
            };

            # The receiver itself, in the user's graphical session (notifications need it).
            systemd.user.services.quickshare-direct = {
              description = "Quick Share receiver";
              wantedBy = [ "graphical-session.target" ];
              partOf = [ "graphical-session.target" ];
              after = [ "graphical-session.target" ];
              unitConfig.ConditionUser = cfg.user;
              path = with pkgs; [ libnotify xdg-utils wl-clipboard xclip networkmanager systemd ];
              environment = {
                QSD_PORT = toString cfg.port;
                QSD_BWU_PORT = toString cfg.upgradePort;
              } // lib.optionalAttrs (cfg.name != null) { QSD_NAME = cfg.name; }
                // lib.optionalAttrs (cfg.downloadDir != null) { QSD_DIR = cfg.downloadDir; }
                // lib.optionalAttrs cfg.classicBluetooth { QSD_BT_CLASSIC = "1"; };
              serviceConfig = {
                ExecStart = "${pkg}/bin/quickshare-direct";
                Restart = "on-failure";
                RestartSec = 5;
              };
            };
          };
        };

      # `nix develop` (or direnv `use flake`): everything needed to build core_lib.
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rustfmt clippy rust-analyzer protobuf pkg-config dbus openssl ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });
    };
}
