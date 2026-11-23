package com.siliconstencil.jafnify;

import android.content.Context;
import android.content.res.Resources;
import android.os.Build;

/**
 * Resolves token colours through the resources, so the current configuration
 * (including values-night) selects the value.
 *
 * A colour resource id is NOT a theme attribute id: obtainStyledAttributes
 * accepts attribute ids and silently returns the fallback for a colour id, so
 * the previous implementation painted every programmatic colour black. Light
 * looked correct only because jafnify_fg is near-black there.
 */
final class Palette {
    private Palette() {}

    /** The colour value of a colour resource, resolved for the current theme. */
    static int colour(Context context, int colourRes) {
        Resources res = context.getResources();
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            return res.getColor(colourRes, context.getTheme());
        }
        return res.getColor(colourRes);
    }
}
