package com.siliconstencil.jafnify;

import android.app.ActionBar;
import android.app.Activity;
import android.graphics.Insets;
import android.os.Build;
import android.util.TypedValue;
import android.view.View;
import android.view.WindowInsets;

/**
 * targetSdk 35 enforces edge-to-edge on Android 15+: the window lays the
 * content out at the top of the screen, so every screen must pad its content
 * by the system bars, the display cutout, and the framework ActionBar height
 * when one is used. One helper for all activities; older releases still fit
 * the window themselves, so nothing is added there.
 *
 * The app draws its own header and the theme has no ActionBar, so the common
 * path pads by the system bars only.
 *
 * The window dump shows content starting at y=0 on API 35, which is why this
 * pads by the raw insets plus the bar height and never by a window offset.
 */
final class SystemBars {
    private SystemBars() {}

    static void apply(final Activity activity) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.VANILLA_ICE_CREAM) {
            return;
        }
        final View root = activity.findViewById(android.R.id.content);
        if (root == null) {
            return;
        }
        final Insets[] last = new Insets[1];
        root.setOnApplyWindowInsetsListener(new View.OnApplyWindowInsetsListener() {
            @Override
            public WindowInsets onApplyWindowInsets(View view, WindowInsets insets) {
                Insets bars = insets.getInsets(
                        WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
                last[0] = bars;
                pad(view, activity, bars);
                return insets;
            }
        });
        // The first dispatch arrives before the framework ActionBar is measured,
        // so it still sees height 0 and falls back to the theme value. Re-apply
        // the padding once layout has measured the bar.
        root.addOnLayoutChangeListener(new View.OnLayoutChangeListener() {
            @Override
            public void onLayoutChange(View view, int left, int top, int right, int bottom,
                    int oldLeft, int oldTop, int oldRight, int oldBottom) {
                if (last[0] != null) {
                    pad(view, activity, last[0]);
                }
            }
        });
        // A static screen may never lay the content view out again after the
        // bar is measured, so the listener above can miss it. A global layout
        // pass always follows the bar's first layout; pad() is a no-op when the
        // value is unchanged, so this cannot loop.
        root.getViewTreeObserver().addOnGlobalLayoutListener(
                new android.view.ViewTreeObserver.OnGlobalLayoutListener() {
                    @Override
                    public void onGlobalLayout() {
                        if (last[0] != null) {
                            pad(root, activity, last[0]);
                        }
                    }
                });
        root.requestApplyInsets();
    }

    // Only reached from apply(), which returns early below API 35.
    @android.annotation.TargetApi(Build.VERSION_CODES.VANILLA_ICE_CREAM)
    private static void pad(View view, Activity activity, Insets bars) {
        int top = topPadding(activity, bars.top);
        if (view.getPaddingLeft() == bars.left && view.getPaddingTop() == top
                && view.getPaddingRight() == bars.right
                && view.getPaddingBottom() == bars.bottom) {
            return;
        }
        view.setPadding(bars.left, top, bars.right, bars.bottom);
    }

    private static int topPadding(Activity activity, int statusTop) {
        ActionBar bar = activity.getActionBar();
        if (bar == null) {
            // A no-action-bar theme: the in-app header follows the status bar.
            return statusTop;
        }
        int measured = bar.isShowing() ? bar.getHeight() : 0;
        if (measured > 0) {
            // On API 35 the ActionBar container starts at the window top, so
            // its measured height is its bottom edge and already includes the
            // status-bar inset. Adding statusTop again counted it twice.
            return Math.max(statusTop, measured);
        }
        return statusTop + themeActionBarHeight(activity);
    }

    private static int themeActionBarHeight(Activity activity) {
        TypedValue value = new TypedValue();
        if (!activity.getTheme().resolveAttribute(
                android.R.attr.actionBarSize, value, true)) {
            return 0;
        }
        return TypedValue.complexToDimensionPixelSize(value.data,
                activity.getResources().getDisplayMetrics());
    }
}
