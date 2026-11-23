package com.siliconstencil.jafnify;

import android.app.Activity;
import android.view.View;
import android.widget.TextView;

/** Wires the shared in-app header: back arrow plus the screen title. */
final class ScreenHeader {
    private ScreenHeader() {}

    static void bind(final Activity activity, int titleRes) {
        bind(activity, activity.getText(titleRes));
    }

    static void bind(final Activity activity, CharSequence title) {
        TextView titleView = activity.findViewById(R.id.tTitle);
        titleView.setText(title);
        View back = activity.findViewById(R.id.bBack);
        back.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                activity.finish();
            }
        });
    }
}
