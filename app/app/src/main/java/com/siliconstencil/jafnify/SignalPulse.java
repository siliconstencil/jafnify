package com.siliconstencil.jafnify;

import android.animation.ObjectAnimator;
import android.content.Context;
import android.provider.Settings;
import android.view.View;

/**
 * The signal arcs of the brand mark carry the session state: a slow, subtle
 * pulse while searching or connecting, steady when connected, muted when
 * there is no session. The system "remove animations" setting turns the pulse
 * into a static arc. Decorative only; the status line carries the meaning.
 */
final class SignalPulse {
    private static final float MUTED = 0.35f;
    private static final long PULSE_MS = 1200L;

    private SignalPulse() {}

    static void apply(View arcs, UiText.Status status, boolean warning) {
        if (arcs == null) {
            return;
        }
        markDetach(arcs);
        boolean active = !warning && (status == UiText.Status.SEARCHING
                || status == UiText.Status.PAIRING
                || status == UiText.Status.CONNECTING
                || status == UiText.Status.RETRYING);
        ObjectAnimator current = (ObjectAnimator) arcs.getTag(R.id.tag_arc_anim);
        if (active && animationsOn(arcs.getContext())) {
            if (current != null && current.isRunning()) {
                return;
            }
            ObjectAnimator pulse = ObjectAnimator.ofFloat(arcs, "alpha", MUTED, 1.0f);
            pulse.setDuration(PULSE_MS);
            pulse.setRepeatMode(ObjectAnimator.REVERSE);
            pulse.setRepeatCount(ObjectAnimator.INFINITE);
            pulse.start();
            arcs.setTag(R.id.tag_arc_anim, pulse);
            return;
        }
        if (current != null) {
            current.cancel();
            arcs.setTag(R.id.tag_arc_anim, null);
        }
        arcs.setAlpha(active || status == UiText.Status.CONNECTED ? 1.0f : MUTED);
    }

    /**
     * Stops the pulse without dimming the mark. The tile home draws its state
     * in the card, and an endless animation would keep the screen from ever
     * going idle (screen readers and UI tests wait for that).
     */
    static void still(View arcs) {
        if (arcs == null) {
            return;
        }
        ObjectAnimator current = (ObjectAnimator) arcs.getTag(R.id.tag_arc_anim);
        if (current != null) {
            current.cancel();
            arcs.setTag(R.id.tag_arc_anim, null);
        }
        if (arcs.getAlpha() != 1.0f) {
            arcs.setAlpha(1.0f);
        }
    }

    private static boolean animationsOn(Context context) {
        float scale = Settings.Global.getFloat(context.getContentResolver(),
                Settings.Global.ANIMATOR_DURATION_SCALE, 1.0f);
        return scale > 0.0f;
    }

    private static void markDetach(final View arcs) {
        if (arcs.getTag(R.id.tag_arc_listener) != null) {
            return;
        }
        arcs.setTag(R.id.tag_arc_listener, Boolean.TRUE);
        arcs.addOnAttachStateChangeListener(new View.OnAttachStateChangeListener() {
            @Override
            public void onViewAttachedToWindow(View view) {
            }

            @Override
            public void onViewDetachedFromWindow(View view) {
                ObjectAnimator running = (ObjectAnimator) view.getTag(R.id.tag_arc_anim);
                if (running != null) {
                    running.cancel();
                    view.setTag(R.id.tag_arc_anim, null);
                }
            }
        });
    }
}
