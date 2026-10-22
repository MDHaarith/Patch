## Install

**Android** — download `patch-android-*.apk` below on your phone and open it
(allow "Install unknown apps" for your browser when asked). Then:

1. Long-press your home screen → *Widgets* → **Patch** to add the home-screen widget.
2. In Patch, Settings → *Display over other apps* if you also want the floating pet
   that hovers above other apps.
3. Settings → *Sync over Tailscale* → enter your hub's address and pairing token.

**Linux (x86_64)** — download `patch-linux-x86_64-*.tar.gz`, then:

```sh
tar xzf patch-linux-x86_64-*.tar.gz && cd patch
install -Dm755 patch patch-hub -t ~/.local/bin/
patch
```

Updates install over the previous version and keep your tasks.
Android only accepts an update signed with the same key. Until the repository
has its release signing key (see the README), nightly APKs use a throwaway test
key, so each nightly needs an uninstall first (sync brings your tasks back).
