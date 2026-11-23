package com.siliconstencil.jafnify;

import android.content.Context;
import android.content.SharedPreferences;

/**
 * Remembers the scope of every tile that can be tapped: the word of the
 * scope, one per tile id, in the app's private preferences. The load/save
 * methods use only {@link Store}, so unit tests need no Android framework;
 * {@link #shared} adapts the real SharedPreferences file.
 *
 * <p>The scope itself, the cycle and the words belong to the core: the phone
 * only stores the choice and hands it in.
 */
public final class TileScopePrefs {
    /** Private preferences file for UI settings (not sharer state). */
    static final String FILE = "jafnify_ui";
    /** The prefix of the key of a tile. */
    static final String PREFIX = "tile_scope_";

    /** The two operations the pure load/save path needs. */
    public interface Store {
        String get(String key);

        void put(String key, String value);
    }

    private TileScopePrefs() {
    }

    /** The key of a tile. */
    public static String key(String tileId) {
        return PREFIX + tileId;
    }

    /** The stored scope of a tile; an empty value means the default. */
    public static String load(Store store, String tileId) {
        if (store == null) {
            return "";
        }
        String word = store.get(key(tileId));
        return word == null ? "" : word;
    }

    /** Stores the word of the scope of a tile. */
    public static void save(Store store, String tileId, String scope) {
        if (store == null) {
            return;
        }
        store.put(key(tileId), scope == null ? "" : scope);
    }

    /** The production Store, backed by the private jafnify_ui preferences. */
    public static Store shared(Context context) {
        final SharedPreferences prefs =
                context.getSharedPreferences(FILE, Context.MODE_PRIVATE);
        return new Store() {
            @Override
            public String get(String key) {
                return prefs.getString(key, null);
            }

            @Override
            public void put(String key, String value) {
                prefs.edit().putString(key, value).apply();
            }
        };
    }
}
