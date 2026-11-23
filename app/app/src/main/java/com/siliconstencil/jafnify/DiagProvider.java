package com.siliconstencil.jafnify;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.content.Context;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;

import java.io.File;
import java.io.FileNotFoundException;

/**
 * Serves the exported {@code cache/diag.zip} for a one-shot share grant.
 *
 * <p>Plain {@link ContentProvider}: the app has no AndroidX, so there is no
 * FileProvider. The provider is {@code exported="false"}; the sending app
 * grants {@code FLAG_GRANT_READ_URI_PERMISSION} for the exact URI, which is
 * enough for the receiving app to read this one file. It never lists the
 * journal files themselves, only the finished archive.
 */
public class DiagProvider extends ContentProvider {
    /** Path segment and cache file name of the shared archive. */
    static final String DIAG_ZIP = DiagExport.ZIP_NAME;

    private File zipFile;

    @Override
    public boolean onCreate() {
        Context context = getContext();
        if (context == null) {
            return false;
        }
        zipFile = new File(context.getCacheDir(), DIAG_ZIP);
        return true;
    }

    @Override
    public String getType(Uri uri) {
        return isDiagZip(uri) ? "application/zip" : null;
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection,
            String[] selectionArgs, String sortOrder) {
        if (!isDiagZip(uri) || zipFile == null || !zipFile.isFile()) {
            return null;
        }
        String[] columns = projection != null ? projection
                : new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE};
        MatrixCursor cursor = new MatrixCursor(columns, 1);
        Object[] row = new Object[columns.length];
        for (int i = 0; i < columns.length; i++) {
            if (OpenableColumns.DISPLAY_NAME.equals(columns[i])) {
                row[i] = DIAG_ZIP;
            } else if (OpenableColumns.SIZE.equals(columns[i])) {
                row[i] = zipFile.length();
            } else {
                row[i] = null;
            }
        }
        cursor.addRow(row);
        return cursor;
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (!isDiagZip(uri)) {
            throw new FileNotFoundException("unknown diagnostic URI: " + uri);
        }
        if (!"r".equals(mode)) {
            throw new FileNotFoundException("diagnostics are read only");
        }
        if (zipFile == null || !zipFile.isFile()) {
            throw new FileNotFoundException("no diagnostic archive");
        }
        return ParcelFileDescriptor.open(zipFile, ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        return null;
    }

    @Override
    public int delete(Uri uri, String selection, String[] selectionArgs) {
        return 0;
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] selectionArgs) {
        return 0;
    }

    private static boolean isDiagZip(Uri uri) {
        return uri != null && DIAG_ZIP.equals(uri.getLastPathSegment());
    }
}
