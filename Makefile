# quickshare-direct: build and install (distribution packages are made from `make install`).
#   make                      build the binary (needs cargo >= 1.85, protoc, libdbus headers)
#   sudo make install         install to /usr/local (PREFIX=/usr for packaging)
#   make install DESTDIR=...  stage for a package
# NixOS: use the flake's module instead.

PREFIX ?= /usr/local
DESTDIR ?=
BINDIR ?= $(PREFIX)/bin
LIBEXECDIR ?= $(PREFIX)/lib/quickshare-direct
DATADIR ?= $(PREFIX)/share
SYSTEMDSYSTEMUNITDIR ?= $(PREFIX)/lib/systemd/system
SYSTEMDUSERUNITDIR ?= $(PREFIX)/lib/systemd/user
POLKITRULESDIR ?= $(DATADIR)/polkit-1/rules.d
NMCONFDIR ?= $(PREFIX)/lib/NetworkManager/conf.d
FIREWALLDDIR ?= $(PREFIX)/lib/firewalld/services
SYSCONFDIR ?= /etc

CARGO ?= cargo
CARGOFLAGS ?= --release --locked
BIN := core_lib/target/release/quickshare-direct
PKG := packaging/linux

.PHONY: all build install uninstall clean

all: build

build:
	cd core_lib && $(CARGO) build $(CARGOFLAGS) --bin quickshare-direct

# Unit files and desktop entries reference the install locations.
define install_subst
	sed -e 's|@BINDIR@|$(BINDIR)|g' -e 's|@LIBEXECDIR@|$(LIBEXECDIR)|g' $(1) > $(2)
endef

install:
	@test -x $(BIN) || { echo "run 'make' first"; exit 1; }
	install -Dm755 $(BIN) $(DESTDIR)$(BINDIR)/quickshare-direct
	install -Dm755 $(PKG)/quickshare-ap $(DESTDIR)$(LIBEXECDIR)/quickshare-ap
	install -Dm755 $(PKG)/quickshare-join $(DESTDIR)$(LIBEXECDIR)/quickshare-join
	install -Dm755 $(PKG)/quickshare-bt-setup $(DESTDIR)$(LIBEXECDIR)/quickshare-bt-setup
	install -d $(DESTDIR)$(SYSTEMDSYSTEMUNITDIR) $(DESTDIR)$(SYSTEMDUSERUNITDIR)
	$(call install_subst,$(PKG)/systemd/quickshare-ap@.service,$(DESTDIR)$(SYSTEMDSYSTEMUNITDIR)/quickshare-ap@.service)
	$(call install_subst,$(PKG)/systemd/quickshare-join@.service,$(DESTDIR)$(SYSTEMDSYSTEMUNITDIR)/quickshare-join@.service)
	$(call install_subst,$(PKG)/systemd/quickshare-bt-connectable.service,$(DESTDIR)$(SYSTEMDSYSTEMUNITDIR)/quickshare-bt-connectable.service)
	$(call install_subst,$(PKG)/systemd/quickshare-direct.service,$(DESTDIR)$(SYSTEMDUSERUNITDIR)/quickshare-direct.service)
	install -Dm644 $(PKG)/polkit/50-quickshare-direct.rules $(DESTDIR)$(POLKITRULESDIR)/50-quickshare-direct.rules
	install -Dm644 $(PKG)/NetworkManager/quickshare-direct.conf $(DESTDIR)$(NMCONFDIR)/quickshare-direct.conf
	install -Dm644 $(PKG)/firewalld/quickshare-direct.xml $(DESTDIR)$(FIREWALLDDIR)/quickshare-direct.xml
	install -Dm644 $(PKG)/ufw/quickshare-direct $(DESTDIR)$(SYSCONFDIR)/ufw/applications.d/quickshare-direct
	install -Dm644 $(PKG)/default.conf $(DESTDIR)$(SYSCONFDIR)/default/quickshare-direct
	install -d $(DESTDIR)$(DATADIR)/applications $(DESTDIR)$(DATADIR)/Thunar/sendto
	$(call install_subst,$(PKG)/quickshare-direct-send.desktop,$(DESTDIR)$(DATADIR)/applications/quickshare-direct-send.desktop)
	$(call install_subst,$(PKG)/quickshare-direct-sendto.desktop,$(DESTDIR)$(DATADIR)/Thunar/sendto/quickshare-direct.desktop)
	install -Dm644 LICENSE $(DESTDIR)$(DATADIR)/licenses/quickshare-direct/LICENSE

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/quickshare-direct
	rm -rf $(DESTDIR)$(LIBEXECDIR)
	rm -f $(DESTDIR)$(SYSTEMDSYSTEMUNITDIR)/quickshare-ap@.service \
	      $(DESTDIR)$(SYSTEMDSYSTEMUNITDIR)/quickshare-join@.service \
	      $(DESTDIR)$(SYSTEMDSYSTEMUNITDIR)/quickshare-bt-connectable.service \
	      $(DESTDIR)$(SYSTEMDUSERUNITDIR)/quickshare-direct.service \
	      $(DESTDIR)$(POLKITRULESDIR)/50-quickshare-direct.rules \
	      $(DESTDIR)$(NMCONFDIR)/quickshare-direct.conf \
	      $(DESTDIR)$(FIREWALLDDIR)/quickshare-direct.xml \
	      $(DESTDIR)$(SYSCONFDIR)/ufw/applications.d/quickshare-direct \
	      $(DESTDIR)$(DATADIR)/applications/quickshare-direct-send.desktop \
	      $(DESTDIR)$(DATADIR)/Thunar/sendto/quickshare-direct.desktop
	rm -rf $(DESTDIR)$(DATADIR)/licenses/quickshare-direct

clean:
	cd core_lib && $(CARGO) clean
