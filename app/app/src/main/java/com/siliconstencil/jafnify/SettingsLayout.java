package com.siliconstencil.jafnify;

/** Small spacing/order decisions kept out of the Activity for JVM tests. */
final class SettingsLayout {
    private SettingsLayout() {
    }

    /** The top margin for a flag row; a bucket note already owns the gap. */
    static int rowTopMarginRes(boolean first, boolean hasNote) {
        if (!first) {
            return R.dimen.jafnify_spacing_sm;
        }
        return hasNote ? R.dimen.jafnify_spacing_xs : R.dimen.jafnify_spacing_md;
    }

    /** Create the Advanced placeholder before the loop when it has no rows. */
    static boolean advancedPlaceholderFirst(boolean hasAdvancedSection,
            boolean hasOtherSections) {
        return !hasAdvancedSection && hasOtherSections;
    }
}
