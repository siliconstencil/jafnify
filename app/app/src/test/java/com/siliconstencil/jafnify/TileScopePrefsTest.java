package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.HashMap;
import java.util.Map;

/**
 * The scope of a tile is remembered per tile: the phone stores the choice and
 * hands it in, the core owns the cycle and the words. A tile that can change
 * its scope is one tap target and says so.
 */
public class TileScopePrefsTest {
    /** A store in memory. */
    private static final class Memory implements TileScopePrefs.Store {
        private final Map<String, String> values = new HashMap<>();

        @Override
        public String get(String key) {
            return values.get(key);
        }

        @Override
        public void put(String key, String value) {
            values.put(key, value);
        }
    }

    private static File locate(String relative) {
        File dir = new File(System.getProperty("user.dir", ".")).getAbsoluteFile();
        for (int i = 0; i < 10 && dir != null; i++) {
            for (String prefix : new String[] {"", "app/app/"}) {
                File found = new File(dir, prefix + relative);
                if (found.exists()) {
                    return found;
                }
            }
            dir = dir.getParentFile();
        }
        throw new AssertionError("not found from " + System.getProperty("user.dir")
                + ": " + relative);
    }

    private static String source(String name) {
        File file = locate("src/main/java/com/siliconstencil/jafnify/" + name);
        try {
            return new String(Files.readAllBytes(file.toPath()),
                    StandardCharsets.UTF_8);
        } catch (IOException e) {
            throw new AssertionError("cannot read " + file, e);
        }
    }

    @Test
    public void theScopeOfATileIsStoredAndReadBack() {
        Memory store = new Memory();
        assertEquals("", TileScopePrefs.load(store, "today"));
        TileScopePrefs.save(store, "today", "SESSION");
        assertEquals("SESSION", TileScopePrefs.load(store, "today"));
    }

    @Test
    public void everyTileHasItsOwnScope() {
        Memory store = new Memory();
        TileScopePrefs.save(store, "today", "SESSION");
        TileScopePrefs.save(store, "speed", "TODAY");
        assertEquals("SESSION", TileScopePrefs.load(store, "today"));
        assertEquals("TODAY", TileScopePrefs.load(store, "speed"));
    }

    @Test
    public void theKeyOfATileIsItsOwn() {
        assertEquals("tile_scope_today", TileScopePrefs.key("today"));
    }

    @Test
    public void theScopesLiveInThePrivateFileOfTheUi() {
        assertEquals("jafnify_ui", TileScopePrefs.FILE);
    }

    @Test
    public void theWholeTileIsTheTargetOfTheTap() {
        String main = source("MainActivity.java");
        assertTrue("the tile itself takes the tap",
                main.contains("views.root.setOnClickListener"));
        assertTrue("the tile is clickable",
                main.contains("views.root.setClickable(true)"));
    }

    @Test
    public void theTapAsksTheCoreForTheNextScopeAndStoresIt() {
        String main = source("MainActivity.java");
        assertTrue("the cycle belongs to the core",
                main.contains("Native.nextTileScope"));
        assertTrue("the phone stores the choice",
                main.contains("TileScopePrefs.save(tileScopes, tileId, next)"));
        assertTrue("the phone hands the scope in",
                main.contains("Native.phoneHome"));
    }

    @Test
    public void theTapHintComesFromTheCore() {
        String main = source("MainActivity.java");
        assertTrue("the words of the hint belong to the core",
                main.contains("Native.tileTapHint()"));
        assertTrue("the tile says that it can be tapped",
                main.contains("setContentDescription(description)"));
    }

    @Test
    public void theTileIsNotAnnouncedAgainOnEveryDraw() {
        // The screen is redrawn every half second. The description may only
        // change when it really changes, or a screen reader reads the tile
        // again and again.
        String main = source("MainActivity.java");
        assertTrue("the old description is read before it is set",
                main.contains("views.root.getContentDescription()"));
    }

    @Test
    public void theTapSurvivesANullFromTheCore() {
        String main = source("MainActivity.java");
        assertTrue("a null from the core does not crash the tap",
                main.contains("next == null"));
    }

    @Test
    public void aTileWithoutAScopeIsNotFocusable() {
        String main = source("MainActivity.java");
        assertTrue("a tile without a scope is not a tap target",
                main.contains("setFocusable(false)"));
    }

    /**
     * The whole tile is a tap target now, so it keeps a finger-sized height.
     * The height comes from the generated tokens: the title line of the tile
     * and the padding around it.
     */
    @Test
    public void theCodeBuiltTileKeepsAFingerSizedHeight() {
        String main = source("MainActivity.java");
        assertTrue("the tile has a minimum height",
                main.contains("tile.setMinimumHeight"));
        int trend = tokenDp("jafnify_line_height_title");
        int pad = tokenDp("jafnify_spacing_md");
        assertTrue("the tile is at least 48 dp high: " + (trend + 2 * pad),
                trend + 2 * pad >= 48);
    }

    /** One size of the generated dimens, in dp. */
    private static int tokenDp(String name) {
        String dimens = read(locate("src/main/res/values/dimens.xml"));
        String marker = "<dimen name=\"" + name + "\">";
        int start = dimens.indexOf(marker);
        assertTrue("the token " + name + " is there", start >= 0);
        int end = dimens.indexOf("</dimen>", start);
        String value = dimens.substring(start + marker.length(), end);
        int number = Integer.parseInt(value.replace("dp", "").replace("sp", "").trim());
        return number;
    }

    private static String read(File file) {
        try {
            return new String(Files.readAllBytes(file.toPath()),
                    StandardCharsets.UTF_8);
        } catch (IOException e) {
            throw new AssertionError("cannot read " + file, e);
        }
    }

    @Test
    public void theNativeDeclarationHasTheScopeArguments() {
        String nativeSource = source("Native.java");
        assertTrue("the scope of the today tile travels",
                nativeSource.contains("String todayScope"));
        assertTrue("the scope of the speed tile travels",
                nativeSource.contains("String speedScope"));
        assertTrue("the session travels",
                nativeSource.contains("String session"));
    }

    @Test
    public void theNetworkPermissionReachesThePhoneHomeCall() {
        String main = source("MainActivity.java");
        assertTrue("the screen reads the phone's permission",
                main.contains("networkMonitor.permissionGranted()"));
        int call = main.indexOf("Native.phoneHome(");
        assertTrue("the home call is there", call >= 0);
        String args = main.substring(call,
                Math.min(main.length(), call + 1200));
        assertTrue("the boolean travels with the home call: " + args,
                args.contains("networkPermission"));
    }

    @Test
    public void theSummaryLineCarriesTheNetworkPermission() {
        String service = source("SharerService.java");
        int call = service.indexOf("Native.phoneStatsLine(");
        assertTrue("the summary-line call is there", call >= 0);
        String args = service.substring(call,
                Math.min(service.length(), call + 1200));
        assertTrue("the permission decides the net capability: " + args,
                args.contains("networkMonitor.permissionGranted()"));
    }

    @Test
    public void theNativeSignaturesDeclareTheNetworkPermission() {
        String nativeSource = source("Native.java");
        assertTrue("the permission is one boolean argument",
                nativeSource.contains("boolean networkPermission"));
    }

    @Test
    public void theSignalPermissionTapOpensSettings() {
        String main = source("MainActivity.java");
        assertTrue("the missing-permission tile has its own tap id",
                main.contains("\"signal-permission\""));
        assertTrue("the tap opens the existing Settings screen",
                main.contains("SettingsActivity.class"));
    }
}
