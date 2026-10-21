package ai.storyteller.vectorcraft;

import android.Manifest;
import android.app.Activity;
import android.app.NativeActivity;
import android.content.ClipData;
import android.content.ContentResolver;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.provider.Settings;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

/**
 * VectorCraft Android activity wrapping {@link NativeActivity} (`libvectorcraft.so`).
 *
 * Requests runtime file/media permissions on startup and bridges system file picker intents
 * ({@link Intent#ACTION_OPEN_DOCUMENT}, {@link Intent#ACTION_CREATE_DOCUMENT},
 * {@link Intent#ACTION_OPEN_DOCUMENT_TREE}) and file-manager reveal intents for the Rust UI.
 */
public final class VectorCraftActivity extends NativeActivity {
    private static final int REQUEST_STORAGE_PERMISSIONS = 1001;
    private static final int REQUEST_PICK_OPEN = 2001;
    private static final int REQUEST_PICK_OPEN_MANY = 2002;
    private static final int REQUEST_PICK_SAVE = 2003;
    private static final int REQUEST_PICK_FOLDER = 2004;

    private volatile boolean bridgeRunning = false;
    private volatile String pendingSaveFallbackName = "Untitled.vc";

    static {
        System.loadLibrary("vectorcraft");
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        requestFilePermissionsIfNeeded();
        startPickerBridge();
    }

    @Override
    protected void onDestroy() {
        bridgeRunning = false;
        super.onDestroy();
    }

    private File ipcDir() {
        File ext = getExternalFilesDir(null);
        if (ext != null && (ext.exists() || ext.mkdirs())) {
            return ext;
        }
        return getFilesDir();
    }

    private void startPickerBridge() {
        if (bridgeRunning) {
            return;
        }
        bridgeRunning = true;
        Thread watcher = new Thread(this::pollPickerRequests, "vc-picker-bridge");
        watcher.setDaemon(true);
        watcher.start();
    }

    private void pollPickerRequests() {
        while (bridgeRunning) {
            try {
                File dir = ipcDir();
                File reqFile = new File(dir, "picker_request.txt");
                if (reqFile.isFile()) {
                    List<String> lines = readLines(reqFile);
                    reqFile.delete();
                    if (!lines.isEmpty()) {
                        String mode = lines.get(0).trim();
                        String arg1 = lines.size() > 1 ? lines.get(1).trim() : "";
                        String arg2 = lines.size() > 2 ? lines.get(2).trim() : "*/*";
                        runOnUiThread(() -> handlePickerRequest(mode, arg1, arg2));
                    }
                }
                Thread.sleep(100);
            } catch (InterruptedException e) {
                break;
            } catch (Exception ignored) {
                // Keep bridge alive across transient I/O errors.
            }
        }
    }

    private void handlePickerRequest(String mode, String nameOrPath, String mime) {
        try {
            String safeMime = (mime == null || mime.isEmpty()) ? "*/*" : mime;
            if ("open".equals(mode)) {
                Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                intent.addCategory(Intent.CATEGORY_OPENABLE);
                intent.setType(safeMime);
                startActivityForResult(intent, REQUEST_PICK_OPEN);
            } else if ("open_many".equals(mode)) {
                Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                intent.addCategory(Intent.CATEGORY_OPENABLE);
                intent.setType(safeMime);
                intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
                startActivityForResult(intent, REQUEST_PICK_OPEN_MANY);
            } else if ("save".equals(mode)) {
                pendingSaveFallbackName = (nameOrPath == null || nameOrPath.isEmpty()) ? "Untitled.vc" : nameOrPath;
                Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
                intent.addCategory(Intent.CATEGORY_OPENABLE);
                intent.setType(safeMime);
                intent.putExtra(Intent.EXTRA_TITLE, pendingSaveFallbackName);
                startActivityForResult(intent, REQUEST_PICK_SAVE);
            } else if ("folder".equals(mode)) {
                Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
                startActivityForResult(intent, REQUEST_PICK_FOLDER);
            } else if ("reveal".equals(mode)) {
                openSystemFileManager(nameOrPath);
            }
        } catch (Exception e) {
            writePickerResponse(new ArrayList<>());
        }
    }

    private void openSystemFileManager(String path) {
        try {
            Intent intent = new Intent(Intent.ACTION_VIEW);
            Uri uri = Uri.parse("content://com.android.externalstorage.documents/root/primary");
            intent.setDataAndType(uri, DocumentsContract.Root.MIME_TYPE_ITEM);
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            startActivity(intent);
        } catch (Exception ignored) {
            try {
                Intent fallback = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                fallback.addCategory(Intent.CATEGORY_OPENABLE);
                fallback.setType("*/*");
                startActivity(fallback);
            } catch (Exception ignoredToo) {
                // No system file manager activity available.
            }
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != REQUEST_PICK_OPEN
                && requestCode != REQUEST_PICK_OPEN_MANY
                && requestCode != REQUEST_PICK_SAVE
                && requestCode != REQUEST_PICK_FOLDER) {
            return;
        }
        List<String> resolved = new ArrayList<>();
        if (resultCode == Activity.RESULT_OK && data != null) {
            if (requestCode == REQUEST_PICK_FOLDER) {
                Uri treeUri = data.getData();
                if (treeUri != null) {
                    String folder = resolveTreeUriToPath(treeUri);
                    if (folder != null && !folder.isEmpty()) {
                        resolved.add(folder);
                    }
                }
            } else if (requestCode == REQUEST_PICK_SAVE) {
                Uri saveUri = data.getData();
                if (saveUri != null) {
                    String savePath = resolveDocumentUriForSave(saveUri, pendingSaveFallbackName);
                    if (savePath != null && !savePath.isEmpty()) {
                        resolved.add(savePath);
                    }
                }
            } else {
                ClipData clip = data.getClipData();
                if (clip != null && clip.getItemCount() > 0) {
                    for (int i = 0; i < clip.getItemCount(); i++) {
                        Uri u = clip.getItemAt(i).getUri();
                        if (u != null) {
                            String p = resolveDocumentUriForOpen(u);
                            if (p != null && !p.isEmpty()) {
                                resolved.add(p);
                            }
                        }
                    }
                } else if (data.getData() != null) {
                    String p = resolveDocumentUriForOpen(data.getData());
                    if (p != null && !p.isEmpty()) {
                        resolved.add(p);
                    }
                }
            }
        }
        writePickerResponse(resolved);
    }

    private String resolveTreeUriToPath(Uri treeUri) {
        try {
            String docId = DocumentsContract.getTreeDocumentId(treeUri);
            String direct = mapDocIdToPath(docId);
            if (direct != null) {
                File f = new File(direct);
                f.mkdirs();
                return f.getAbsolutePath();
            }
        } catch (Exception ignored) {
        }
        File fallback = new File(Environment.getExternalStorageDirectory(), "Documents/VectorCraft");
        fallback.mkdirs();
        return fallback.getAbsolutePath();
    }

    private String resolveDocumentUriForOpen(Uri uri) {
        try {
            if ("file".equalsIgnoreCase(uri.getScheme())) {
                return uri.getPath();
            }
            if (DocumentsContract.isDocumentUri(this, uri)) {
                String docId = DocumentsContract.getDocumentId(uri);
                String direct = mapDocIdToPath(docId);
                if (direct != null) {
                    File f = new File(direct);
                    if (f.isFile() && f.canRead()) {
                        return f.getAbsolutePath();
                    }
                }
            }
            String displayName = queryDisplayName(uri, "imported_file");
            File cacheDir = new File(ipcDir(), "Picked");
            cacheDir.mkdirs();
            File dest = new File(cacheDir, sanitizeFileName(displayName));
            try (InputStream in = getContentResolver().openInputStream(uri);
                 OutputStream out = new FileOutputStream(dest)) {
                if (in == null) {
                    return null;
                }
                byte[] buf = new byte[65536];
                int n;
                while ((n = in.read(buf)) > 0) {
                    out.write(buf, 0, n);
                }
            }
            return dest.getAbsolutePath();
        } catch (Exception e) {
            return null;
        }
    }

    private String resolveDocumentUriForSave(Uri uri, String fallbackName) {
        try {
            if ("file".equalsIgnoreCase(uri.getScheme())) {
                return uri.getPath();
            }
            if (DocumentsContract.isDocumentUri(this, uri)) {
                String docId = DocumentsContract.getDocumentId(uri);
                String direct = mapDocIdToPath(docId);
                if (direct != null) {
                    File f = new File(direct);
                    File parent = f.getParentFile();
                    if (parent != null) {
                        parent.mkdirs();
                    }
                    return f.getAbsolutePath();
                }
            }
            String displayName = queryDisplayName(uri, fallbackName);
            File cacheDir = new File(ipcDir(), "Picked");
            cacheDir.mkdirs();
            File staged = new File(cacheDir, sanitizeFileName(displayName));
            if (staged.exists()) {
                staged.delete();
            }
            watchAndSyncSaveUri(staged, uri);
            return staged.getAbsolutePath();
        } catch (Exception e) {
            return null;
        }
    }

    private void watchAndSyncSaveUri(File staged, Uri targetUri) {
        Thread syncThread = new Thread(() -> {
            long deadline = System.currentTimeMillis() + 120_000L;
            long lastModified = 0L;
            while (System.currentTimeMillis() < deadline) {
                try {
                    if (staged.isFile() && staged.length() > 0 && staged.lastModified() != lastModified) {
                        lastModified = staged.lastModified();
                        Thread.sleep(150);
                        ContentResolver resolver = getContentResolver();
                        try (InputStream in = new FileInputStream(staged);
                             OutputStream out = resolver.openOutputStream(targetUri, "wt")) {
                            if (out != null) {
                                byte[] buf = new byte[65536];
                                int n;
                                while ((n = in.read(buf)) > 0) {
                                    out.write(buf, 0, n);
                                }
                                out.flush();
                            }
                        }
                        break;
                    }
                    Thread.sleep(150);
                } catch (Exception ignored) {
                }
            }
        }, "vc-save-uri-sync");
        syncThread.setDaemon(true);
        syncThread.start();
    }

    private String mapDocIdToPath(String docId) {
        if (docId == null || docId.isEmpty()) {
            return null;
        }
        if (docId.startsWith("raw:")) {
            return docId.substring(4);
        }
        int colon = docId.indexOf(':');
        if (colon > 0) {
            String type = docId.substring(0, colon);
            String rel = docId.substring(colon + 1);
            if ("primary".equalsIgnoreCase(type)) {
                File root = Environment.getExternalStorageDirectory();
                return rel.isEmpty() ? root.getAbsolutePath() : new File(root, rel).getAbsolutePath();
            }
            if ("home".equalsIgnoreCase(type)) {
                File docs = new File(Environment.getExternalStorageDirectory(), "Documents");
                return rel.isEmpty() ? docs.getAbsolutePath() : new File(docs, rel).getAbsolutePath();
            }
        }
        return null;
    }

    private String queryDisplayName(Uri uri, String fallback) {
        try (Cursor cursor = getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                String name = cursor.getString(0);
                if (name != null && !name.trim().isEmpty()) {
                    return name.trim();
                }
            }
        } catch (Exception ignored) {
        }
        return fallback;
    }

    private static String sanitizeFileName(String name) {
        String cleaned = name.replaceAll("[\\\\/:*?\"<>|]", "_").trim();
        return cleaned.isEmpty() ? "Untitled.vc" : cleaned;
    }

    private static List<String> readLines(File file) {
        List<String> out = new ArrayList<>();
        try (BufferedReader reader = new BufferedReader(new InputStreamReader(new FileInputStream(file), StandardCharsets.UTF_8))) {
            String line;
            while ((line = reader.readLine()) != null) {
                out.add(line);
            }
        } catch (Exception ignored) {
        }
        return out;
    }

    private void writePickerResponse(List<String> paths) {
        try {
            File dir = ipcDir();
            File tmp = new File(dir, "picker_response.tmp");
            File dst = new File(dir, "picker_response.txt");
            StringBuilder sb = new StringBuilder();
            for (String p : paths) {
                sb.append(p).append('\n');
            }
            try (OutputStream out = new FileOutputStream(tmp)) {
                out.write(sb.toString().getBytes(StandardCharsets.UTF_8));
                out.flush();
            }
            if (dst.exists()) {
                dst.delete();
            }
            tmp.renameTo(dst);
        } catch (Exception ignored) {
        }
    }

    private void requestFilePermissionsIfNeeded() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            List<String> needed = new ArrayList<>();
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                addIfNotGranted(needed, Manifest.permission.READ_MEDIA_IMAGES);
                addIfNotGranted(needed, Manifest.permission.READ_MEDIA_VIDEO);
                addIfNotGranted(needed, Manifest.permission.READ_MEDIA_AUDIO);
            } else {
                addIfNotGranted(needed, Manifest.permission.READ_EXTERNAL_STORAGE);
                addIfNotGranted(needed, Manifest.permission.WRITE_EXTERNAL_STORAGE);
            }
            if (!needed.isEmpty()) {
                requestPermissions(needed.toArray(new String[0]), REQUEST_STORAGE_PERMISSIONS);
            }
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && !Environment.isExternalStorageManager()) {
            try {
                Intent intent = new Intent(
                    Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                    Uri.parse("package:" + getPackageName())
                );
                startActivity(intent);
            } catch (Exception ignored) {
                try {
                    Intent fallback = new Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION);
                    startActivity(fallback);
                } catch (Exception ignoredToo) {
                    // Device does not expose the All Files Access settings screen.
                }
            }
        }
    }

    private void addIfNotGranted(List<String> list, String permission) {
        if (checkSelfPermission(permission) != PackageManager.PERMISSION_GRANTED) {
            list.add(permission);
        }
    }
}
