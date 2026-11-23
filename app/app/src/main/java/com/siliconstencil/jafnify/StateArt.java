package com.siliconstencil.jafnify;

/**
 * The Android side of the core's state -> glyph vocabulary
 * ({@code jafnify_core::snapshot::state_glyph}). Every state has a stable
 * name, the shape name from the shared J-Signal set, and the drawable named
 * after that shape. No second vocabulary is invented here: the names match
 * the core exactly, so a Windows or iOS port maps the same strings.
 */
final class StateArt {
    final String glyphName;
    final String shapeName;
    final String drawableName;
    final int drawableRes;
    final int colourRes;

    private StateArt(String glyphName, String shapeName, int drawableRes, int colourRes) {
        this.glyphName = glyphName;
        this.shapeName = shapeName;
        this.drawableName = "ic_state_" + shapeName.replace('-', '_');
        this.drawableRes = drawableRes;
        this.colourRes = colourRes;
    }

    /** The glyph for a UI state. UNKNOWN falls back to the idle marker. */
    static StateArt of(UiText.Status status) {
        switch (status) {
            case PAIRING:
                return new StateArt("pairing", "one-arc-hollow-dot",
                        R.drawable.ic_state_one_arc_hollow_dot, R.color.jafnify_wait);
            case CONNECTING:
                return new StateArt("connecting", "one-arc-filled-dot",
                        R.drawable.ic_state_one_arc_filled_dot, R.color.jafnify_wait);
            case CONNECTED:
                return new StateArt("connected", "two-arcs-filled-dot",
                        R.drawable.ic_state_two_arcs_filled_dot, R.color.jafnify_ok);
            case RETRYING:
                return new StateArt("retrying", "one-arc-hollow-dot-slash",
                        R.drawable.ic_state_one_arc_hollow_dot_slash, R.color.jafnify_wait);
            case SEARCHING:
                return new StateArt("searching", "one-arc-hollow-dot",
                        R.drawable.ic_state_one_arc_hollow_dot, R.color.jafnify_muted);
            case IDLE:
            case UNKNOWN:
            default:
                return new StateArt("idle", "no-arc-hollow-dot",
                        R.drawable.ic_state_no_arc_hollow_dot, R.color.jafnify_muted);
        }
    }

    /** The error marker, which keeps the existing exclamation shape. */
    static StateArt error() {
        return new StateArt("error", "exclamation",
                R.drawable.ic_state_exclamation, R.color.jafnify_err);
    }
}
