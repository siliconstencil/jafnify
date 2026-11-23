package com.siliconstencil.jafnify;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.DialogInterface;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.graphics.Typeface;
import android.net.ConnectivityManager;
import android.net.Uri;
import android.os.BatteryManager;
import android.os.Build;
import android.os.Bundle;
import android.os.PowerManager;
import android.provider.Settings;
import android.text.InputType;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.widget.Button;
import android.widget.CompoundButton;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.RadioGroup;
import android.widget.Switch;
import android.widget.TextView;
import android.widget.Toast;

import java.io.File;
import java.util.List;
import java.util.Locale;

/** Theme, start-on-open and the notification note, moved off the home card. */
public class SettingsActivity extends Activity {
    /** The runtime request code for the phone's network permission. */
    private static final int REQUEST_PHONE_STATE = 41;
    /** The auto-revoke page action; Settings has no such constant. */
    private ThemeMode appliedTheme;
    private SharingActions sharing;
    private TextView unusedState;
    private TextView keepBatteryState;
    private TextView dataSaverState;
    private TextView phoneBatteryLine;
    /** The row that asked for the permission; the answer belongs to it. */
    private String pendingPermissionRow;
    /** The switches of the rows that need the phone's network permission. */
    private final java.util.Map<String, Switch> permissionSwitches = new java.util.HashMap<>();

    @Override
    protected void attachBaseContext(Context base) {
        appliedTheme = ThemePrefs.load(ThemePrefs.shared(base));
        super.attachBaseContext(ThemeMode.wrap(base, appliedTheme));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_settings);
        ScreenHeader.bind(this, R.string.settings_title);
        SystemBars.apply(this);

        sharing = new SharingActions(this);

        RadioGroup themeGroup = findViewById(R.id.themeGroup);
        themeGroup.check(themeButton(ThemePrefs.load(ThemePrefs.shared(this))));
        themeGroup.setOnCheckedChangeListener(new RadioGroup.OnCheckedChangeListener() {
            @Override
            public void onCheckedChanged(RadioGroup group, int checkedId) {
                ThemeMode.apply(SettingsActivity.this, themeForButton(checkedId));
            }
        });

        CompoundButton startOnOpen = findViewById(R.id.swStartOnOpen);
        startOnOpen.setChecked(Autostart.load(Autostart.shared(this)));
        startOnOpen.setOnCheckedChangeListener(new CompoundButton.OnCheckedChangeListener() {
            @Override
            public void onCheckedChanged(CompoundButton buttonView, boolean isChecked) {
                // Saving only; it takes effect on the next cold open.
                Autostart.save(Autostart.shared(SettingsActivity.this), isChecked);
            }
        });

        nicknameField = findViewById(R.id.etNickname);
        nicknameField.setText(nicknamePrefs().getString(NICKNAME_KEY, ""));

        findViewById(R.id.bNotifSettings).setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                SharingActions.openNotificationSettings(SettingsActivity.this);
            }
        });
        findViewById(R.id.bHomeLayout).setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                startActivity(new Intent(SettingsActivity.this,
                        HomeLayoutActivity.class));
            }
        });
        renderFlags();
        refreshKeepRunning();
        refreshNotifNote();
    }

    @Override
    protected void onPause() {
        super.onPause();
        if (nicknameField != null) {
            nicknamePrefs()
                    .edit()
                    .putString(NICKNAME_KEY,
                            DeviceNickname.clean(nicknameField.getText().toString()))
                    .apply();
        }
    }

    /** The private preferences this screen keeps its own values in. */
    private android.content.SharedPreferences nicknamePrefs() {
        return getSharedPreferences(Autostart.FILE, MODE_PRIVATE);
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (refreshTheme()) {
            return;
        }
        refreshKeepRunning();
        refreshNotifNote();
    }

    /** Recreates when another screen changed the stored theme. */
    private boolean refreshTheme() {
        if (isFinishing()) {
            return false;
        }
        if (ThemePrefs.load(ThemePrefs.shared(this)) != appliedTheme) {
            recreate();
            return true;
        }
        return false;
    }

    /** The name this device calls itself; local only, never sent to the peer. */
    private static final String NICKNAME_KEY = "device_nickname";
    private android.widget.EditText nicknameField;

    private void refreshNotifNote() {
        sharing.refreshNotifNote(findViewById(R.id.notifBanner));
    }

    /** Adds the shared flag rows; an empty table adds no view. */
    private void renderFlags() {
        Autostart.Store store = Autostart.shared(this);
        List<FeatureFlags.Section> sections = FeatureFlags.screen(
                Native.flagsMeta(), Native.flagsBucketsMeta(), store);
        LinearLayout main = findViewById(R.id.flagSettings);
        LinearLayout groups = findViewById(R.id.flagGroups);
        LinearLayout advanced = null;
        boolean hasAdvanced = false;
        for (FeatureFlags.Section section : sections) {
            if (section.placement != FeatureFlags.Placement.MAIN
                    && FeatureFlags.ADVANCED.equals(section.bucket)) {
                hasAdvanced = true;
            }
        }
        if (SettingsLayout.advancedPlaceholderFirst(hasAdvanced, !sections.isEmpty())) {
            FeatureFlags.BucketInfo info = bucketInfo(FeatureFlags.ADVANCED);
            if (info != null) {
                advanced = addGroupHeader(groups, info.title,
                        info.collapsedByDefault);
            }
        }
        for (FeatureFlags.Section section : sections) {
            if (section.placement == FeatureFlags.Placement.MAIN) {
                addRows(main, section, store);
            } else {
                LinearLayout content = addGroup(groups, section, store);
                if (FeatureFlags.ADVANCED.equals(section.bucket)) {
                    advanced = content;
                }
            }
        }
        if (advanced == null) {
            FeatureFlags.BucketInfo info = bucketInfo(FeatureFlags.ADVANCED);
            if (info != null) {
                advanced = addGroupHeader(groups, info.title,
                        info.collapsedByDefault);
            }
        }
        if (advanced != null) {
            addKeepRunning(advanced);
        }
    }

    private FeatureFlags.BucketInfo bucketInfo(String key) {
        for (FeatureFlags.BucketInfo info
                : FeatureFlags.parseBuckets(Native.flagsBucketsMeta())) {
            if (key.equals(info.key)) {
                return info;
            }
        }
        return null;
    }

    private void addRows(LinearLayout parent, FeatureFlags.Section section,
            Autostart.Store store) {
        for (int i = 0; i < section.rows.size(); i++) {
            addRow(parent, section.rows.get(i), store,
                    i == 0 ? R.dimen.jafnify_spacing_md : R.dimen.jafnify_spacing_sm);
        }
    }

    private void addRow(LinearLayout parent, FeatureFlags.Row row,
            Autostart.Store store, int topMarginRes) {
        Switch toggle = new Switch(this);
        toggle.setText(row.label);
        toggle.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        toggle.setMinHeight(dp(48));
        toggle.setMinWidth(dp(48));
        boolean value = row.value;
        if (value && isPhonePermissionRow(row.id) && !phoneStateGranted()) {
            // Revoked in Android settings (Android kills the app, so the
            // service cannot do it): the switch shows the truth.
            value = false;
            store.putBoolean(row.id, false);
        }
        toggle.setChecked(value);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        params.topMargin = getResources().getDimensionPixelSize(topMarginRes);
        parent.addView(toggle, params);

        TextView help = new TextView(this);
        help.setText(row.help);
        help.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        help.setTextColor(Palette.colour(this, R.color.jafnify_muted));
        LinearLayout.LayoutParams helpParams = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        helpParams.topMargin = getResources().getDimensionPixelSize(
                R.dimen.jafnify_spacing_xs);
        parent.addView(help, helpParams);

        LinearLayout extras = null;
        if ("battery_status".equals(row.id)) {
            extras = addBatteryStatus(parent, row.value);
        } else if ("data_budget".equals(row.id)) {
            extras = addBudgetControls(parent);
        }
        final LinearLayout extraBox = extras;
        if (extraBox != null) {
            extraBox.setVisibility(row.value ? View.VISIBLE : View.GONE);
        }
        if (isPhonePermissionRow(row.id)) {
            permissionSwitches.put(row.id, toggle);
        }
        toggle.setOnCheckedChangeListener(new CompoundButton.OnCheckedChangeListener() {
            @Override
            public void onCheckedChanged(CompoundButton buttonView, boolean isChecked) {
                // Already granted (for example by the other row): no dialog.
                if (isChecked && isPhonePermissionRow(row.id) && !phoneStateGranted()) {
                    buttonView.setChecked(false);
                    askPhonePermission(row.id);
                    return;
                }
                store.putBoolean(row.id, isChecked);
                if (extraBox != null) {
                    extraBox.setVisibility(isChecked ? View.VISIBLE : View.GONE);
                }
                if (isChecked && "battery_status".equals(row.id)) {
                    return;
                }
            }
        });
    }

    /** The rows that need the phone's network permission. */
    private boolean isPhonePermissionRow(String id) {
        return FlagValues.NETWORK_DETAILS.equals(id) || "best_spot".equals(id);
    }

    /** The one-line explanation, then the system dialog. Never at startup. */
    private void askPhonePermission(String rowId) {
        pendingPermissionRow = rowId;
        if (Build.VERSION.SDK_INT < PhonePermission.MIN_SDK) {
            storePhonePermission(false);
            return;
        }
        new AlertDialog.Builder(this)
                .setTitle(R.string.permission_network_title)
                .setMessage(R.string.permission_network_message)
                .setPositiveButton(R.string.permission_network_continue,
                        new DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(DialogInterface dialog, int which) {
                                requestPermissions(
                                        new String[] {
                                                android.Manifest.permission.READ_PHONE_STATE },
                                        REQUEST_PHONE_STATE);
                            }
                        })
                .setNegativeButton(R.string.cancel, null)
                .show();
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions,
            int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode != REQUEST_PHONE_STATE) {
            return;
        }
        boolean granted = grantResults.length > 0
                && grantResults[0] == PackageManager.PERMISSION_GRANTED;
        boolean rationale = Build.VERSION.SDK_INT >= 23
                && shouldShowRequestPermissionRationale(
                        android.Manifest.permission.READ_PHONE_STATE);
        PhonePermission.State state = PhonePermission.afterRequest(Build.VERSION.SDK_INT,
                granted, rationale);
        storePhonePermission(state == PhonePermission.State.GRANTED);
        if (state != PhonePermission.State.GRANTED) {
            showPhonePermissionReason(state);
        }
    }

    /** The switch follows the permission: granted means on, otherwise off. */
    private void storePhonePermission(boolean on) {
        String row = pendingPermissionRow;
        pendingPermissionRow = null;
        if (row == null) {
            return;
        }
        Switch toggle = permissionSwitches.get(row);
        if (on && toggle != null) {
            // The switch shows the truth; its listener stores the value.
            toggle.setChecked(true);
            return;
        }
        Autostart.shared(this).putBoolean(row, on);
    }

    private boolean phoneStateGranted() {
        return Build.VERSION.SDK_INT >= PhonePermission.MIN_SDK
                && checkSelfPermission(android.Manifest.permission.READ_PHONE_STATE)
                        == PackageManager.PERMISSION_GRANTED;
    }

    /** The one reason line; the blocked case also links to the settings page. */
    private void showPhonePermissionReason(PhonePermission.State state) {
        String reason = PhonePermission.reasonLine(state,
                getString(R.string.permission_network_denied),
                getString(R.string.permission_network_settings),
                getString(R.string.permission_network_revoked),
                getString(R.string.permission_network_unsupported));
        if (reason.isEmpty()) {
            return;
        }
        if (!PhonePermission.needsSettings(state)) {
            Toast.makeText(this, reason, Toast.LENGTH_LONG).show();
            return;
        }
        new AlertDialog.Builder(this)
                .setMessage(reason)
                .setPositiveButton(R.string.permission_network_open_settings,
                        new DialogInterface.OnClickListener() {
                            @Override
                            public void onClick(DialogInterface dialog, int which) {
                                openAppSettings();
                            }
                        })
                .setNegativeButton(R.string.cancel, null)
                .show();
    }

    /** The app's own system settings page; the user taps the link. */
    private void openAppSettings() {
        try {
            Intent intent = new Intent(
                    Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                    Uri.fromParts("package", getPackageName(), null));
            startActivity(intent);
        } catch (ActivityNotFoundException e) {
            Toast.makeText(this, R.string.permission_network_no_settings,
                    Toast.LENGTH_LONG).show();
        }
    }

    /** One static line: the live battery values moved to the main screen. */
    private LinearLayout addBatteryStatus(LinearLayout parent, boolean on) {
        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);
        phoneBatteryLine = new TextView(this);
        phoneBatteryLine.setText(R.string.battery_shown_on_main);
        phoneBatteryLine.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        phoneBatteryLine.setTextColor(Palette.colour(this, R.color.jafnify_muted));
        LinearLayout.LayoutParams lineParams = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        lineParams.topMargin = getResources().getDimensionPixelSize(
                R.dimen.jafnify_spacing_xs);
        box.addView(phoneBatteryLine, lineParams);
        parent.addView(box);
        return box;
    }

    /** The allowance and reset day under the switch; invalid input is a line. */
    private LinearLayout addBudgetControls(LinearLayout parent) {
        UsageLineStore store = new UsageLineStore(
                new File(getFilesDir(), "phone-stats.tsv"));
        String record = store.loadBudget();
        String initial = record == null
                ? Native.budgetRecord(0, 0, 1, 0, 0)
                : record;

        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);

        final TextView status = new TextView(this);
        status.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        status.setTextColor(Palette.colour(this, R.color.jafnify_muted));
        status.setText(Native.budgetDisplay(initial));
        box.addView(status);

        final TextView warning = new TextView(this);
        warning.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        warning.setTextColor(Palette.colour(this, R.color.jafnify_muted));
        String warningText = Native.budgetWarning(initial);
        warning.setText(warningText);
        warning.setVisibility(warningText == null || warningText.isEmpty()
                ? View.GONE
                : View.VISIBLE);
        box.addView(warning);

        long allow = budgetField(initial, "allow", 0);
        int reset = (int) budgetField(initial, "reset", 1);

        final EditText allowance = new EditText(this);
        allowance.setHint(R.string.budget_allowance_hint);
        allowance.setInputType(InputType.TYPE_CLASS_NUMBER
                | InputType.TYPE_NUMBER_FLAG_DECIMAL);
        allowance.setSingleLine(true);
        if (allow > 0) {
            allowance.setText(String.format(Locale.US, "%.1f",
                    allow / 1_000_000_000.0));
        }
        box.addView(allowance);

        final EditText resetDay = new EditText(this);
        resetDay.setHint(R.string.budget_reset_hint);
        resetDay.setInputType(InputType.TYPE_CLASS_NUMBER);
        resetDay.setSingleLine(true);
        if (reset > 0) {
            resetDay.setText(String.valueOf(reset));
        }
        box.addView(resetDay);

        final TextView error = new TextView(this);
        error.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        error.setTextColor(Palette.colour(this, R.color.jafnify_muted));
        box.addView(error);

        Button save = new Button(this);
        save.setText(R.string.budget_save);
        save.setMinHeight(dp(48));
        box.addView(save);
        save.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                long newAllow = budgetField(store.loadBudget(), "allow", 0);
                String allowText = allowance.getText().toString().trim();
                if (!allowText.isEmpty()) {
                    try {
                        double gb = Double.parseDouble(allowText);
                        if (!(gb >= 0 && gb <= 100_000)) {
                            throw new NumberFormatException();
                        }
                        newAllow = Math.round(gb * 1_000_000_000.0);
                    } catch (NumberFormatException e) {
                        error.setText(R.string.budget_invalid);
                        return;
                    }
                }
                int newReset = (int) budgetField(
                        store.loadBudget(), "reset", 1);
                String resetText = resetDay.getText().toString().trim();
                if (!resetText.isEmpty()) {
                    try {
                        newReset = Integer.parseInt(resetText);
                        if (newReset < 1 || newReset > 31) {
                            throw new NumberFormatException();
                        }
                    } catch (NumberFormatException e) {
                        error.setText(R.string.budget_invalid);
                        return;
                    }
                }
                String base = store.loadBudget();
                String line = Native.budgetRecord(
                        newAllow,
                        budgetField(base, "used", 0),
                        newReset,
                        (int) budgetField(base, "month", 0),
                        (int) budgetField(base, "warned", 0));
                store.saveBudget(line);
                error.setText("");
                status.setText(Native.budgetDisplay(line));
                String due = Native.budgetWarning(line);
                warning.setText(due);
                warning.setVisibility(due == null || due.isEmpty()
                        ? View.GONE
                        : View.VISIBLE);
            }
        });
        parent.addView(box);
        return box;
    }

    private static long budgetField(String line, String key, long fallback) {
        if (line == null) {
            return fallback;
        }
        for (String part : line.trim().split(" ")) {
            String[] pair = part.split("=", 2);
            if (pair.length == 2 && key.equals(pair[0])) {
                try {
                    return Long.parseLong(pair[1]);
                } catch (NumberFormatException e) {
                    return fallback;
                }
            }
        }
        return fallback;
    }

    private LinearLayout addGroup(LinearLayout parent,
            FeatureFlags.Section section, Autostart.Store store) {
        LinearLayout content = addGroupHeader(parent, section.title,
                section.collapsedByDefault);

        if (section.note != null) {
            TextView note = new TextView(this);
            note.setText(section.note);
            note.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                    getResources().getDimension(R.dimen.jafnify_type_body));
            note.setTextColor(Palette.colour(this, R.color.jafnify_muted));
            LinearLayout.LayoutParams noteParams = new LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.MATCH_PARENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT);
            noteParams.topMargin = getResources().getDimensionPixelSize(
                    R.dimen.jafnify_spacing_xs);
            content.addView(note, noteParams);
        }
        for (int i = 0; i < section.rows.size(); i++) {
            addRow(content, section.rows.get(i), store,
                    SettingsLayout.rowTopMarginRes(i == 0, section.note != null));
        }

        return content;
    }

    private LinearLayout addGroupHeader(LinearLayout parent, String title,
            boolean collapsedByDefault) {
        TextView header = new TextView(this);
        header.setText(title);
        header.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_caption));
        header.setTextColor(Palette.colour(this, R.color.jafnify_fg));
        header.setTypeface(Typeface.DEFAULT_BOLD);
        header.setClickable(true);
        header.setFocusable(true);
        header.setMinHeight(dp(48));
        // The 48dp touch target stays; the title sits in its middle so the
        // spare height does not show as a gap under the title.
        header.setGravity(Gravity.CENTER_VERTICAL | Gravity.START);
        LinearLayout.LayoutParams headerParams = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        headerParams.topMargin = getResources().getDimensionPixelSize(
                R.dimen.jafnify_spacing_xl);
        parent.addView(header, headerParams);

        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setVisibility(collapsedByDefault ? View.GONE : View.VISIBLE);
        parent.addView(content);

        header.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                boolean open = content.getVisibility() == View.VISIBLE;
                content.setVisibility(open ? View.GONE : View.VISIBLE);
            }
        });
        return content;
    }

    /** The three keep-running rows: status only, each with its own Open link. */
    private void addKeepRunning(LinearLayout content) {
        TextView title = new TextView(this);
        title.setText(R.string.keep_running_title);
        title.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        title.setTextColor(Palette.colour(this, R.color.jafnify_fg));
        title.setMinHeight(dp(48));
        LinearLayout.LayoutParams titleParams = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        titleParams.topMargin = getResources().getDimensionPixelSize(
                R.dimen.jafnify_spacing_sm);
        content.addView(title, titleParams);

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            unusedState = addKeepRow(content, R.string.keep_unused_label,
                    new Runnable() {
                        @Override
                        public void run() {
                            openUnusedSettings();
                        }
                    });
        }
        keepBatteryState = addKeepRow(content, R.string.keep_battery_label,
                new Runnable() {
                    @Override
                    public void run() {
                        openAppSettings();
                    }
                });
        dataSaverState = addKeepRow(content, R.string.keep_data_saver_label,
                new Runnable() {
                    @Override
                    public void run() {
                        openDataSaverSettings();
                    }
                });
    }

    private TextView addKeepRow(LinearLayout content, int labelRes, final Runnable open) {
        TextView label = new TextView(this);
        label.setText(labelRes);
        label.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        label.setTextColor(Palette.colour(this, R.color.jafnify_fg));
        LinearLayout.LayoutParams labelParams = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        labelParams.topMargin = getResources().getDimensionPixelSize(
                R.dimen.jafnify_spacing_sm);
        content.addView(label, labelParams);

        TextView status = new TextView(this);
        status.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        status.setTextColor(Palette.colour(this, R.color.jafnify_muted));
        LinearLayout.LayoutParams statusParams = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        statusParams.topMargin = getResources().getDimensionPixelSize(
                R.dimen.jafnify_spacing_xs);
        content.addView(status, statusParams);

        TextView link = new TextView(this);
        link.setText(R.string.keep_open);
        link.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        link.setTextColor(Palette.colour(this, R.color.jafnify_fg));
        link.setClickable(true);
        link.setFocusable(true);
        link.setMinHeight(dp(48));
        link.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                open.run();
            }
        });
        LinearLayout.LayoutParams linkParams = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        linkParams.topMargin = getResources().getDimensionPixelSize(
                R.dimen.jafnify_spacing_xs);
        content.addView(link, linkParams);
        return status;
    }

    private void refreshKeepRunning() {
        int sdk = Build.VERSION.SDK_INT;
        if (unusedState != null) {
            boolean whitelisted = sdk >= Build.VERSION_CODES.R
                    && getPackageManager().isAutoRevokeWhitelisted();
            int state = KeepRunning.unused(sdk, whitelisted);
            if (state == KeepRunning.UNUSED_OK) {
                unusedState.setText(R.string.keep_unused_ok);
            } else if (state == KeepRunning.UNUSED_CAN_REVOKE) {
                unusedState.setText(R.string.keep_unused_can_revoke);
            } else {
                unusedState.setText("");
            }
        }
        if (keepBatteryState != null) {
            boolean ignoring = false;
            if (sdk >= Build.VERSION_CODES.M) {
                PowerManager power = (PowerManager) getSystemService(POWER_SERVICE);
                ignoring = power != null
                        && power.isIgnoringBatteryOptimizations(getPackageName());
            }
            keepBatteryState.setText(ignoring
                    ? R.string.keep_battery_ok : R.string.keep_battery_optimised);
        }
        if (dataSaverState != null) {
            int status = 0;
            if (sdk >= Build.VERSION_CODES.N) {
                ConnectivityManager cm = (ConnectivityManager)
                        getSystemService(CONNECTIVITY_SERVICE);
                status = cm == null ? 0 : cm.getRestrictBackgroundStatus();
            }
            int state = KeepRunning.dataSaver(status);
            if (state == KeepRunning.DATA_OK) {
                dataSaverState.setText(R.string.keep_data_ok);
            } else if (state == KeepRunning.DATA_BLOCKED) {
                dataSaverState.setText(R.string.keep_data_blocked);
            } else {
                dataSaverState.setText("");
            }
        }
    }

    private Uri packageUri() {
        return Uri.parse("package:" + getPackageName());
    }

    private Intent appDetailsIntent() {
        return new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, packageUri());
    }

    private void openUnusedSettings() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            openAppSettings();
            return;
        }
        openOrFallback(new Intent(Intent.ACTION_AUTO_REVOKE_PERMISSIONS, packageUri()));
    }

    private void openDataSaverSettings() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) {
            openAppSettings();
            return;
        }
        openOrFallback(new Intent(
                Settings.ACTION_IGNORE_BACKGROUND_DATA_RESTRICTIONS_SETTINGS,
                packageUri()));
    }

    private void openOrFallback(Intent intent) {
        if (!openIntent(intent)) {
            openIntent(appDetailsIntent());
        }
    }

    private boolean openIntent(Intent intent) {
        try {
            startActivity(intent);
            return true;
        } catch (ActivityNotFoundException e) {
            Toast.makeText(this, R.string.battery_settings_missing,
                    Toast.LENGTH_SHORT).show();
            return false;
        }
    }

    private int dp(int value) {
        return (int) (value * getResources().getDisplayMetrics().density + 0.5f);
    }

    /** Opens the notification note target from a deep link if one appears. */
    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        refreshNotifNote();
    }

    private static int themeButton(ThemeMode mode) {
        switch (mode) {
            case DARK:
                return R.id.themeDark;
            case LIGHT:
                return R.id.themeLight;
            default:
                return R.id.themeSystem;
        }
    }

    private static ThemeMode themeForButton(int id) {
        if (id == R.id.themeDark) {
            return ThemeMode.DARK;
        }
        if (id == R.id.themeLight) {
            return ThemeMode.LIGHT;
        }
        return ThemeMode.SYSTEM;
    }
}
