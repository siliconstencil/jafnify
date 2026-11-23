package com.siliconstencil.jafnify;

import android.app.Activity;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.net.Uri;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.text.Selection;
import android.text.Spannable;
import android.view.View;
import android.widget.Button;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.io.File;
import java.util.ArrayList;
import java.util.List;

/**
 * The old details panel: current-session stats, the stored history and the
 * service log, plus the D3 copy/share/save actions. The export goes through
 * the core's alias redaction exactly as it did on the home screen.
 */
public class DiagnosticsActivity extends Activity {
    private static final int LOG_TAIL_LINES = 50;
    private static final int HISTORY_LINES = 50;
    // Diagnostics changes slowly; a fast poll only kept the window busy and
    // made uiautomator (and the battery) suffer.
    private static final long POLL_MS = 2000;

    private TextView tDetails;
    private TextView tHistory;
    private TextView tLog;
    private SharerService service;
    private boolean bound = false;
    private UiText.Parsed parsed = UiText.parse(null);
    private ThemeMode appliedTheme;
    private boolean resumed = false;
    private String lastDetails;
    private String lastHistory;
    private String lastLog;

    private final Handler poller = new Handler(Looper.getMainLooper());
    private final Runnable refresh = new Runnable() {
        @Override
        public void run() {
            if (!resumed) {
                return;
            }
            if (service != null) {
                parsed = UiText.parse(service.getStatusRaw());
            }
            render();
            poller.postDelayed(this, POLL_MS);
        }
    };

    private final ServiceConnection connection = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName name, IBinder binder) {
            service = ((SharerService.LocalBinder) binder).getService();
            if (resumed) {
                poller.removeCallbacks(refresh);
                poller.post(refresh);
            }
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {
            service = null;
        }
    };

    @Override
    protected void attachBaseContext(Context base) {
        appliedTheme = ThemePrefs.load(ThemePrefs.shared(base));
        super.attachBaseContext(ThemeMode.wrap(base, appliedTheme));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_diagnostics);
        ScreenHeader.bind(this, R.string.diagnostics_title);
        SystemBars.apply(this);

        // The body is selectable text inside a ScrollView, so a focused child
        // would make the view scroll to it and open the screen part-way down.
        // The scroller takes focus itself (and is focusable in the layout) and
        // starts at offset 0 on a fresh open.
        final ScrollView scroller = findViewById(R.id.diagScroll);
        scroller.requestFocus();
        scroller.post(new Runnable() {
            @Override
            public void run() {
                scroller.scrollTo(0, 0);
            }
        });

        tDetails = findViewById(R.id.tDetails);
        tHistory = findViewById(R.id.tHistory);
        tLog = findViewById(R.id.tLog);
        Button copyLog = findViewById(R.id.bCopyLog);
        Button shareLog = findViewById(R.id.bShareLog);
        Button saveDiag = findViewById(R.id.bSaveDiag);
        copyLog.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                copyLog();
            }
        });
        shareLog.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                shareLog();
            }
        });
        saveDiag.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                saveDiagnostics();
            }
        });
        render();
    }

    @Override
    protected void onStart() {
        super.onStart();
        bound = bindService(new Intent(this, SharerService.class),
                connection, Context.BIND_AUTO_CREATE);
    }

    @Override
    protected void onResume() {
        super.onResume();
        Clock.follow(this);
        resumed = true;
        poller.removeCallbacks(refresh);
        poller.post(refresh);
    }

    @Override
    protected void onPause() {
        resumed = false;
        poller.removeCallbacks(refresh);
        super.onPause();
    }

    @Override
    protected void onStop() {
        poller.removeCallbacks(refresh);
        if (bound) {
            unbindService(connection);
            bound = false;
        }
        service = null;
        super.onStop();
    }

    /** Current-session stats, the stored history and the service log tail. */
    private void render() {
        List<String> detail = new ArrayList<>();
        SessionRecord current = service == null ? null : service.getCurrentSession();
        if (current == null) {
            detail.add(getString(R.string.no_current_session));
        } else {
            detail.addAll(StatsText.details(current));
        }
        detail.add("");
        detail.add(getString(R.string.details_status, orNone(parsed.line)));
        String detailsText = joinLines(detail);
        setTextIfChanged(tDetails, detailsText, lastDetails);
        lastDetails = detailsText;

        List<SessionRecord> history =
                service == null ? new ArrayList<SessionRecord>() : service.getSessionHistory();
        String historyText;
        if (history.isEmpty()) {
            historyText = getString(R.string.history_empty);
        } else {
            List<String> rows = new ArrayList<>();
            for (int i = 0; i < history.size() && i < HISTORY_LINES; i++) {
                rows.add(StatsText.historyLine(history.get(i)));
            }
            historyText = joinLines(rows);
        }
        setTextIfChanged(tHistory, historyText, lastHistory);
        lastHistory = historyText;

        List<String> logTail =
                service == null ? new ArrayList<String>() : service.getLogTail(LOG_TAIL_LINES);
        String logText = logTail.isEmpty() ? getString(R.string.log_idle) : joinLines(logTail);
        setTextIfChanged(tLog, logText, lastLog);
        lastLog = logText;
    }

    /**
     * A TextView setText marks the window busy even when the string is equal,
     * so the poll would never let the UI go idle. Only touch the view when the
     * rendered text actually changed.
     */
    private static void setTextIfChanged(TextView view, String text, String previous) {
        if (!text.equals(previous)) {
            view.setText(text);
            // A selectable TextView puts the cursor at offset 0 on setText and
            // then scrolls that point into view, which pulled the ScrollView
            // back to the top on every update while the user read further
            // down. With no cursor there is nothing to bring into view; a long
            // press still selects text as before.
            CharSequence shown = view.getText();
            if (shown instanceof Spannable) {
                Selection.removeSelection((Spannable) shown);
            }
        }
    }

    private void copyLog() {
        ClipboardManager clipboard =
                (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        clipboard.setPrimaryClip(ClipData.newPlainText(
                getString(R.string.log_title), logForSharing()));
        Toast.makeText(this, R.string.log_copied, Toast.LENGTH_SHORT).show();
    }

    private void shareLog() {
        Intent send = new Intent(Intent.ACTION_SEND);
        send.setType("text/plain");
        send.putExtra(Intent.EXTRA_TEXT, logForSharing());
        startActivity(Intent.createChooser(send, getString(R.string.share_log_title)));
    }

    /**
     * Zip the diagnostic journal and share it through the app's own provider.
     * Every step is optional: no running service, no archive, or no provider
     * shows a short message instead of crashing.
     */
    private void saveDiagnostics() {
        if (service == null) {
            Toast.makeText(this, R.string.save_diag_failed, Toast.LENGTH_SHORT).show();
            return;
        }
        File zip = service.exportDiagZip(exportAliases());
        if (zip == null) {
            Toast.makeText(this, R.string.save_diag_failed, Toast.LENGTH_SHORT).show();
            return;
        }
        String authority = getPackageName() + ".diag";
        if (getPackageManager().resolveContentProvider(authority, 0) == null) {
            Toast.makeText(this, R.string.save_diag_failed, Toast.LENGTH_SHORT).show();
            return;
        }
        Uri uri = Uri.parse("content://" + authority + "/" + DiagExport.ZIP_NAME);
        Intent send = new Intent(Intent.ACTION_SEND);
        send.setType("application/zip");
        send.putExtra(Intent.EXTRA_STREAM, uri);
        send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        try {
            startActivity(Intent.createChooser(send, getString(R.string.save_diag_title)));
        } catch (RuntimeException e) {
            Toast.makeText(this, R.string.save_diag_failed, Toast.LENGTH_SHORT).show();
        }
    }

    private String logForSharing() {
        String text = service == null ? "" : service.getLogText();
        return text.isEmpty()
                ? getString(R.string.log_idle)
                : ConnectionLog.exportText(text, exportAliases());
    }

    /**
     * The peer aliases the app actually has: the stored display name, the live
     * name from the native status, and the 12-char prefix of the peer key.
     * One peer per line, its spellings separated by tabs.
     */
    private String exportAliases() {
        List<String> aliases = new ArrayList<>();
        addExportAlias(aliases, Native.pairedName(Native.ensure(this)));
        addExportAlias(aliases, parsed.peerName);
        if (parsed.peerId != null && parsed.peerId.length() >= 12) {
            addExportAlias(aliases, parsed.peerId.substring(0, 12));
        }
        if (aliases.isEmpty()) {
            return "";
        }
        StringBuilder out = new StringBuilder();
        for (String alias : aliases) {
            if (out.length() > 0) {
                out.append('\t');
            }
            out.append(alias);
        }
        return out.toString();
    }

    private static void addExportAlias(List<String> aliases, String value) {
        if (value == null) {
            return;
        }
        String alias = value.trim();
        if (alias.isEmpty() || aliases.contains(alias)) {
            return;
        }
        if (alias.indexOf('\t') >= 0 || alias.indexOf('\n') >= 0
                || alias.indexOf('\r') >= 0) {
            return;
        }
        aliases.add(alias);
    }

    private static String joinLines(List<String> lines) {
        StringBuilder out = new StringBuilder();
        for (String line : lines) {
            if (out.length() > 0) {
                out.append('\n');
            }
            out.append(line);
        }
        return out.toString();
    }

    private String orNone(String s) {
        return s == null || s.isEmpty() ? getString(R.string.details_none) : s;
    }
}
