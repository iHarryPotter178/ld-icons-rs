PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share/ld-icons
DESTDIR ?=

CARGO ?= cargo

INSTALL ?= install
RM ?= rm -rf

.PHONY: all install uninstall reinstall

all:
	$(CARGO) build --release

install:
	$(INSTALL) -d "$(DESTDIR)$(DATADIR)"
	$(INSTALL) -d "$(DESTDIR)$(BINDIR)"
	$(CARGO) build --release
	$(INSTALL) -m 755 target/release/ld-icons "$(DESTDIR)$(BINDIR)/ldicons"
	$(INSTALL) -m 644 ldicons.conf "$(DESTDIR)$(DATADIR)/ldicons.conf"
	$(INSTALL) -m 644 README.md "$(DESTDIR)$(DATADIR)/README.md"

uninstall:
	$(RM) "$(DESTDIR)$(BINDIR)/ldicons"
	$(RM) "$(DESTDIR)$(DATADIR)"

reinstall: uninstall install
