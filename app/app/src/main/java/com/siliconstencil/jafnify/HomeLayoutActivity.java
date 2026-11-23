package com.siliconstencil.jafnify;

import android.app.Activity;
import android.content.Context;
import android.os.Bundle;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.widget.AdapterView;
import android.widget.ArrayAdapter;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.Spinner;
import android.widget.TextView;

import java.util.ArrayList;
import java.util.List;

/**
 * The Home layout screen: the grid size, where the wide tile sits in a 7
 * grid, and one picker per slot. Every change is saved through the core and
 * applies on the home at once; a "Done" button only leaves the screen.
 */
public class HomeLayoutActivity extends Activity {
    private ThemeMode appliedTheme;
    private HomeLayout layout;
    private List<HomeLayoutPrefs.Option> catalog = new ArrayList<>();
    private Spinner gridSpinner;
    private Spinner wideSpinner;
    private View wideRow;
    private LinearLayout slotList;
    private final List<Spinner> slotSpinners = new ArrayList<>();
    /** While true, programmatic picker changes must not save anything. */
    private boolean rebuilding;

    @Override
    protected void attachBaseContext(Context base) {
        appliedTheme = ThemePrefs.load(ThemePrefs.shared(base));
        super.attachBaseContext(ThemeMode.wrap(base, appliedTheme));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_home_layout);
        ScreenHeader.bind(this, R.string.home_layout_title);
        SystemBars.apply(this);

        catalog = HomeLayoutPrefs.catalog();
        layout = HomeLayout.parse(HomeLayoutPrefs.loadText(this));

        LinearLayout body = findViewById(R.id.layoutBody);
        gridSpinner = addChoiceRow(body, R.string.home_layout_grid, gridLabels());
        gridSpinner.setSelection(gridIndex(layout.grid));
        gridSpinner.setOnItemSelectedListener(new AdapterView.OnItemSelectedListener() {
            @Override
            public void onItemSelected(AdapterView<?> parent, View view, int position,
                    long id) {
                if (rebuilding) {
                    return;
                }
                int grid = HomeLayout.GRIDS[position];
                if (grid == layout.grid) {
                    return;
                }
                layout.grid = grid;
                applyFromRows();
            }

            @Override
            public void onNothingSelected(AdapterView<?> parent) {
            }
        });

        wideSpinner = addChoiceRow(body, R.string.home_layout_wide, wideLabels());
        wideRow = (View) wideSpinner.getParent();
        wideSpinner.setSelection(wideIndex(layout.wide));
        wideSpinner.setOnItemSelectedListener(new AdapterView.OnItemSelectedListener() {
            @Override
            public void onItemSelected(AdapterView<?> parent, View view, int position,
                    long id) {
                if (rebuilding) {
                    return;
                }
                String wide = position == 1 ? HomeLayout.BOTTOM : HomeLayout.TOP;
                if (wide.equals(layout.wide)) {
                    return;
                }
                layout.wide = wide;
                applyFromRows();
            }

            @Override
            public void onNothingSelected(AdapterView<?> parent) {
            }
        });

        slotList = new LinearLayout(this);
        slotList.setOrientation(LinearLayout.VERTICAL);
        body.addView(slotList, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));

        Button done = new Button(this);
        done.setText(R.string.home_layout_done);
        done.setMinHeight(dp(48));
        LinearLayout.LayoutParams doneParams = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        doneParams.topMargin = dp(24);
        done.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                finish();
            }
        });
        body.addView(done, doneParams);

        rebuildSlots();
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (appliedTheme != null
                && ThemePrefs.load(ThemePrefs.shared(this)) != appliedTheme) {
            recreate();
        }
    }

    /** A titled picker row; returns the Spinner's row. */
    private Spinner addChoiceRow(LinearLayout parent, int titleRes, String[] labels) {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        TextView title = new TextView(this);
        title.setText(titleRes);
        title.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        title.setTextColor(Palette.colour(this, R.color.jafnify_fg));
        row.addView(title, new LinearLayout.LayoutParams(
                0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f));
        Spinner spinner = new Spinner(this);
        ArrayAdapter<String> adapter = new ArrayAdapter<>(this,
                android.R.layout.simple_spinner_item, labels);
        adapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        spinner.setAdapter(adapter);
        spinner.setMinimumWidth(dp(96));
        row.addView(spinner, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        params.topMargin = dp(16);
        parent.addView(row, params);
        return spinner;
    }

    private String[] gridLabels() {
        return new String[] {"6", "7", "8"};
    }

    private String[] wideLabels() {
        return new String[] {
                getString(R.string.home_layout_wide_top),
                getString(R.string.home_layout_wide_bottom)};
    }

    private static int gridIndex(int grid) {
        for (int i = 0; i < HomeLayout.GRIDS.length; i++) {
            if (HomeLayout.GRIDS[i] == grid) {
                return i;
            }
        }
        return HomeLayout.GRIDS.length - 1;
    }

    private static int wideIndex(String wide) {
        return HomeLayout.BOTTOM.equals(wide) ? 1 : 0;
    }

    /** Rebuilds the slot pickers from the current layout. */
    private void rebuildSlots() {
        rebuilding = true;
        wideRow.setVisibility(layout.grid == 7 ? View.VISIBLE : View.GONE);
        gridSpinner.setSelection(gridIndex(layout.grid));
        wideSpinner.setSelection(wideIndex(layout.wide));
        slotList.removeAllViews();
        slotSpinners.clear();
        int wideSlot = layout.wideSlot();
        List<HomeLayoutPrefs.Option> options = options();
        for (int i = 0; i < layout.slotCount(); i++) {
            slotList.addView(slotRow(i, wideSlot == i, options));
        }
        rebuilding = false;
    }

    private View slotRow(int index, boolean wide, List<HomeLayoutPrefs.Option> options) {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);

        TextView title = new TextView(this);
        title.setText(wide
                ? getString(R.string.home_layout_slot_wide, index + 1)
                : getString(R.string.home_layout_slot, index + 1));
        title.setTextSize(TypedValue.COMPLEX_UNIT_PX,
                getResources().getDimension(R.dimen.jafnify_type_body));
        title.setTextColor(Palette.colour(this, R.color.jafnify_muted));
        row.addView(title, new LinearLayout.LayoutParams(
                0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f));

        final Spinner spinner = new Spinner(this);
        String[] labels = new String[options.size()];
        int selected = 0;
        String current = index < layout.slots.size() ? layout.slots.get(index) : "";
        for (int i = 0; i < options.size(); i++) {
            labels[i] = options.get(i).label;
            if (options.get(i).id.equals(current)) {
                selected = i;
            }
        }
        ArrayAdapter<String> adapter = new ArrayAdapter<>(this,
                android.R.layout.simple_spinner_item, labels);
        adapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        spinner.setAdapter(adapter);
        spinner.setSelection(selected);
        spinner.setMinimumWidth(dp(120));
        spinner.setOnItemSelectedListener(new AdapterView.OnItemSelectedListener() {
            @Override
            public void onItemSelected(AdapterView<?> parent, View view, int position,
                    long id) {
                if (rebuilding) {
                    return;
                }
                int slot = slotSpinners.indexOf(spinner);
                if (slot < 0 || position >= options.size()) {
                    return;
                }
                String picked = options.get(position).id;
                if (slot < layout.slots.size() && picked.equals(layout.slots.get(slot))) {
                    return;
                }
                applyFromRows();
            }

            @Override
            public void onNothingSelected(AdapterView<?> parent) {
            }
        });
        row.addView(spinner, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        slotSpinners.add(spinner);

        Button up = moveButton(R.string.home_layout_up, index > 0, index, -1);
        row.addView(up);
        Button down = moveButton(R.string.home_layout_down,
                index < layout.slotCount() - 1, index, 1);
        row.addView(down);

        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        params.topMargin = dp(8);
        row.setLayoutParams(params);
        return row;
    }

    private Button moveButton(int textRes, boolean enabled, final int index,
            final int delta) {
        Button button = new Button(this);
        button.setText(textRes);
        button.setMinHeight(dp(48));
        button.setMinWidth(dp(48));
        button.setEnabled(enabled);
        button.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                List<String> ids = currentIds();
                int other = index + delta;
                if (other < 0 || other >= ids.size()) {
                    return;
                }
                String moved = ids.get(index);
                ids.set(index, ids.get(other));
                ids.set(other, moved);
                saveIds(ids);
                rebuildSlots();
            }
        });
        return button;
    }

    /** The current slot ids, read from the pickers. */
    private List<String> currentIds() {
        List<HomeLayoutPrefs.Option> options = options();
        List<String> ids = new ArrayList<>();
        for (Spinner spinner : slotSpinners) {
            int position = spinner.getSelectedItemPosition();
            if (position >= 0 && position < options.size()) {
                ids.add(options.get(position).id);
            }
        }
        return ids;
    }

    /** Saves the pickers through the core, then redraws the slot list. */
    private void applyFromRows() {
        saveIds(currentIds());
        rebuildSlots();
    }

    /** Normalizes the ids with the core, stores the text and reloads it. */
    private void saveIds(List<String> ids) {
        String text = HomeLayoutPrefs.normalize(layout.grid, layout.wide, ids);
        if (!text.isEmpty() && !text.equals(HomeLayoutPrefs.loadText(this))) {
            HomeLayoutPrefs.saveText(this, text);
        }
        layout = HomeLayout.parse(text);
    }

    /** The picker options: every placeable tile, or the current ids as a fallback. */
    private List<HomeLayoutPrefs.Option> options() {
        if (!catalog.isEmpty()) {
            return catalog;
        }
        List<HomeLayoutPrefs.Option> options = new ArrayList<>();
        for (String id : layout.slots) {
            options.add(new HomeLayoutPrefs.Option(id, id.toUpperCase(java.util.Locale.ROOT)));
        }
        return options;
    }

    private int dp(int value) {
        return Math.round(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP,
                value, getResources().getDisplayMetrics()));
    }
}
