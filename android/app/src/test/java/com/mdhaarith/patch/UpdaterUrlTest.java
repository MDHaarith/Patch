package com.mdhaarith.patch;

import java.net.URL;

/** Standalone check: run with -ea and android.jar on the classpath. */
public final class UpdaterUrlTest {
    public static void main(String[] args) throws Exception {
        for (String url : new String[] {
                "https://github.com/MDHaarith/Patch/releases/download/v1/patch.apk",
                "https://release-assets.githubusercontent.com/asset?token=abc",
                "https://objects.githubusercontent.com:443/asset",
                "https://GITHUB.COM/asset"}) {
            assert Updater.allowedDownload(new URL(url)) : url;
        }
        for (String url : new String[] {
                "http://github.com/asset", "https://github.com.evil.test/asset",
                "https://evilgithubusercontent.com/asset", "https://githubusercontent.com/asset",
                "https://github.com@evil.test/asset", "https://evil@github.com/asset",
                "https://github.com:8443/asset", "https://127.0.0.1/asset", "file:///tmp/asset.apk"}) {
            assert !Updater.allowedDownload(new URL(url)) : url;
        }
        System.out.println("Updater URL checks passed");
    }
}
