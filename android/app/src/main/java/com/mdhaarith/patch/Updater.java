package com.mdhaarith.patch;

import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageInstaller;
import android.net.Uri;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.util.Log;
import android.widget.Toast;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.atomic.AtomicBoolean;

public final class Updater extends BroadcastReceiver {
    private static final AtomicBoolean busy = new AtomicBoolean();

    static void check(Context context) {
        if (!busy.compareAndSet(false, true)) {
            toast(context, "Update check already running");
            return;
        }
        try {
            if (!context.getPackageManager().canRequestPackageInstalls()) {
                toast(context, "Allow Patch to install updates, then tap Check for updates again");
                context.startActivity(new Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                        Uri.parse("package:" + context.getPackageName())).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
                busy.set(false);
                return;
            }
        } catch (RuntimeException e) {
            busy.set(false);
            toast(context, "Cannot open update permission: " + e.getMessage());
            return;
        }
        toast(context, "Checking for updates…");
        new Thread(() -> {
            try {
                JSONObject release = latest();
                String tag = release.getString("tag_name");
                if (tag.equals(BuildConfig.RELEASE_TAG)) {
                    toast(context, "Patch is up to date");
                    return;
                }
                JSONArray assets = release.getJSONArray("assets");
                for (int i = 0; i < assets.length(); i++) {
                    JSONObject asset = assets.getJSONObject(i);
                    String name = asset.getString("name");
                    if (name.startsWith("patch-android-") && name.endsWith(".apk")) {
                        toast(context, "Downloading " + tag + "…");
                        install(context, new URL(asset.getString("browser_download_url")), asset.getLong("size"));
                        return;
                    }
                }
                throw new IOException("No Android APK in the latest release");
            } catch (Exception e) {
                Log.w("Patch", "update failed", e);
                toast(context, "Update failed: " + e.getMessage());
            } finally {
                busy.set(false);
            }
        }, "patch-update").start();
    }

    private static JSONObject latest() throws Exception {
        HttpURLConnection connection = connect(new URL("https://api.github.com/repos/MDHaarith/Patch/releases/latest"));
        connection.setRequestProperty("Accept", "application/vnd.github+json");
        try {
            requireOk(connection);
            try (InputStream in = connection.getInputStream(); ByteArrayOutputStream out = new ByteArrayOutputStream()) {
                byte[] buffer = new byte[8192];
                int count;
                while ((count = in.read(buffer)) != -1) {
                    if (out.size() + count > 1024 * 1024) {
                        throw new IOException("Release metadata is too large");
                    }
                    out.write(buffer, 0, count);
                }
                return new JSONObject(new String(out.toByteArray(), StandardCharsets.UTF_8));
            }
        } finally {
            connection.disconnect();
        }
    }

    static boolean allowedDownload(URL url) {
        String host = url.getHost();
        return "https".equals(url.getProtocol()) && url.getUserInfo() == null
                && (url.getPort() == -1 || url.getPort() == 443)
                && ("github.com".equalsIgnoreCase(host)
                    || host.toLowerCase(java.util.Locale.ROOT).endsWith(".githubusercontent.com"));
    }

    private static HttpURLConnection download(URL url) throws IOException {
        for (int redirects = 0; redirects <= 5; redirects++) {
            if (!allowedDownload(url)) {
                throw new IOException("Untrusted APK download URL");
            }
            HttpURLConnection connection = connect(url);
            try {
                int status = connection.getResponseCode();
                if (status == 301 || status == 302 || status == 303 || status == 307 || status == 308) {
                    String location = connection.getHeaderField("Location");
                    if (location == null) {
                        throw new IOException("Missing APK redirect URL");
                    }
                    url = new URL(url, location);
                    connection.disconnect();
                    continue;
                }
                requireOk(connection);
                return connection;
            } catch (IOException | RuntimeException e) {
                connection.disconnect();
                throw e;
            }
        }
        throw new IOException("Too many APK redirects");
    }

    private static HttpURLConnection connect(URL url) throws IOException {
        HttpURLConnection connection = (HttpURLConnection) url.openConnection();
        connection.setInstanceFollowRedirects(false);
        connection.setConnectTimeout(15000);
        connection.setReadTimeout(30000);
        connection.setRequestProperty("User-Agent", "Patch-Android/" + BuildConfig.VERSION_NAME);
        return connection;
    }

    private static void requireOk(HttpURLConnection connection) throws IOException {
        if (connection.getResponseCode() != HttpURLConnection.HTTP_OK) {
            throw new IOException("GitHub returned HTTP " + connection.getResponseCode());
        }
    }

    private static void install(Context context, URL url, long size) throws IOException {
        if (size <= 0) {
            throw new IOException("Invalid APK size");
        }
        PackageInstaller installer = context.getPackageManager().getPackageInstaller();
        PackageInstaller.SessionParams params = new PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL);
        params.setAppPackageName(context.getPackageName());
        params.setSize(size);
        int id = installer.createSession(params);
        boolean committed = false;
        try (PackageInstaller.Session session = installer.openSession(id)) {
            HttpURLConnection connection = download(url);
            try (InputStream in = connection.getInputStream(); OutputStream out = session.openWrite("base.apk", 0, size)) {
                byte[] buffer = new byte[65536];
                long total = 0;
                int count;
                while ((count = in.read(buffer)) != -1) {
                    total += count;
                    if (total > size) {
                        throw new IOException("APK exceeds release size");
                    }
                    out.write(buffer, 0, count);
                }
                if (total != size) {
                    throw new IOException("Incomplete APK download");
                }
                session.fsync(out);
            } finally {
                connection.disconnect();
            }
            // The system fills in status and confirmation extras on this explicit callback.
            int flags = PendingIntent.FLAG_UPDATE_CURRENT;
            if (Build.VERSION.SDK_INT >= 31) {
                flags |= PendingIntent.FLAG_MUTABLE;
            }
            Intent result = new Intent(context, Updater.class);
            PendingIntent callback = PendingIntent.getBroadcast(context, id, result, flags);
            session.commit(callback.getIntentSender());
            committed = true;
        } finally {
            if (!committed) {
                installer.abandonSession(id);
            }
        }
    }

    @Override
    public void onReceive(Context context, Intent intent) {
        int status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE);
        if (status == PackageInstaller.STATUS_PENDING_USER_ACTION) {
            Intent confirm = intent.getParcelableExtra(Intent.EXTRA_INTENT);
            try {
                if (confirm == null) {
                    throw new IllegalStateException("Missing installer confirmation");
                }
                context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            } catch (RuntimeException e) {
                toast(context, "Cannot open installer: " + e.getMessage());
            }
        } else if (status == PackageInstaller.STATUS_SUCCESS) {
            toast(context, "Patch updated");
        } else {
            toast(context, "Install failed: " + intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE));
        }
    }

    private static void toast(Context context, String message) {
        new Handler(Looper.getMainLooper()).post(() -> Toast.makeText(context, message, Toast.LENGTH_LONG).show());
    }
}
