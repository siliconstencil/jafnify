package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.w3c.dom.NodeList;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;

import javax.xml.parsers.DocumentBuilderFactory;

/**
 * Reads the layouts and Java sources as text, the same way the core token
 * test reads the generated resources. Pure JVM: no device, no AndroidX.
 */
public class LayoutContractTest {
    private static final String ANDROID = "http://schemas.android.com/apk/res/android";

    // ---- locating the module on disk --------------------------------------

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

    private static File resDir() {
        return locate("src/main/res");
    }

    private static File javaDir() {
        return locate("src/main/java/com/siliconstencil/jafnify");
    }

    private static List<File> layouts() {
        File[] files = new File(resDir(), "layout").listFiles();
        assertTrue("layout directory has files", files != null && files.length > 0);
        List<File> out = new ArrayList<>();
        for (File f : files) {
            if (f.getName().endsWith(".xml")) {
                out.add(f);
            }
        }
        return out;
    }

    private static Document parse(File file) {
        try {
            DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
            factory.setNamespaceAware(true);
            return factory.newDocumentBuilder().parse(file);
        } catch (Exception e) {
            throw new AssertionError("cannot parse " + file, e);
        }
    }

    private static String attr(Element e, String name) {
        String value = e.getAttributeNS(ANDROID, name);
        return value == null || value.isEmpty() ? null : value;
    }

    private static List<Element> elements(File file) {
        List<Element> out = new ArrayList<>();
        collect(parse(file).getDocumentElement(), out);
        return out;
    }

    private static void collect(Element e, List<Element> out) {
        out.add(e);
        NodeList children = e.getChildNodes();
        for (int i = 0; i < children.getLength(); i++) {
            Node node = children.item(i);
            if (node instanceof Element) {
                collect((Element) node, out);
            }
        }
    }

    private static boolean isClickable(Element e) {
        String tag = e.getTagName();
        if (tag.equals("Button") || tag.equals("ImageButton")
                || tag.equals("CheckBox") || tag.equals("RadioButton")) {
            return true;
        }
        return "true".equals(attr(e, "clickable")) || attr(e, "onClick") != null;
    }

    private static Element byIdIn(Element root, String id) {
        List<Element> all = new ArrayList<>();
        collect(root, all);
        for (Element e : all) {
            if (("@" + "+id/" + id).equals(attr(e, "id"))) {
                return e;
            }
        }
        return null;
    }

    private static Element lastElementChild(Element parent) {
        Element last = null;
        NodeList children = parent.getChildNodes();
        for (int i = 0; i < children.getLength(); i++) {
            if (children.item(i) instanceof Element) {
                last = (Element) children.item(i);
            }
        }
        return last;
    }

    private static String sourceOf(File file) {
        try {
            return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
        } catch (IOException e) {
            throw new AssertionError("cannot read " + file, e);
        }
    }

    /**
     * The first argument of a call in the source: the key or the name. It
     * reads the shape of the code, not a line of it, so a refactoring that
     * keeps the behaviour does not break the contract.
     */
    private static String firstArgument(String source, String call) {
        String arguments = callArguments(source, call);
        if (arguments == null) {
            return null;
        }
        int comma = arguments.indexOf(',');
        return (comma < 0 ? arguments : arguments.substring(0, comma)).trim();
    }

    /** The arguments of the first call of a name, or null when there is none. */
    private static String callArguments(String source, String call) {
        int at = source.indexOf(call);
        if (at < 0) {
            return null;
        }
        int open = source.indexOf('(', at);
        int close = source.indexOf(')', open);
        if (open < 0 || close < 0) {
            return null;
        }
        return source.substring(open + 1, close);
    }

    /** The variable a call is assigned to, or null when it is not assigned. */
    private static String assignedVariable(String source, String call) {
        int at = source.indexOf(call);
        if (at < 0) {
            return null;
        }
        int start = Math.max(source.lastIndexOf(';', at), source.lastIndexOf('{', at));
        int equals = source.lastIndexOf('=', at);
        if (equals < start) {
            return null;
        }
        String head = source.substring(start + 1, equals).trim();
        int space = head.lastIndexOf(' ');
        return space < 0 ? head : head.substring(space + 1);
    }

    // ---- touch targets -----------------------------------------------------

    @Test
    public void everyClickableViewHasA48dpMinimumInEveryLayout() {
        for (File layout : layouts()) {
            for (Element e : elements(layout)) {
                if (!isClickable(e)) {
                    continue;
                }
                String where = layout.getName() + " " + e.getTagName()
                        + (attr(e, "id") == null ? "" : " " + attr(e, "id"));
                assertEquals(where + " minHeight", "48dp", attr(e, "minHeight"));
                assertEquals(where + " minWidth", "48dp", attr(e, "minWidth"));
            }
        }
    }

    @Test
    public void buttonsUseTheSharedPrimaryAndTextStyles() {
        for (File layout : layouts()) {
            for (Element e : elements(layout)) {
                String tag = e.getTagName();
                if (!tag.equals("Button")) {
                    continue;
                }
                String where = layout.getName() + " " + attr(e, "id");
                // The one exception: the three tile-home buttons
                // share the row in equal boxes.
                if ("1".equals(attr(e, "layout_weight"))) {
                    assertEquals(where + " weighted width", "0dp", attr(e, "layout_width"));
                } else {
                    assertEquals(where + " width", "wrap_content", attr(e, "layout_width"));
                }
                String style = e.getAttribute("style");
                if ("@style/JafnifyTextButton".equals(style)) {
                    assertNull(where + " text action is flush with the body",
                            attr(e, "paddingStart"));
                    assertEquals(where + " keeps the 48dp target",
                            "48dp", attr(e, "minHeight"));
                } else {
                    assertEquals(where + " is the one primary style",
                            "@style/JafnifyPrimaryButton", style);
                }
            }
        }
    }

    @Test
    public void thePairingCodeIsACardWithOneFilledAction() {
        Element card = byId("activity_main.xml", "pairCard");
        assertNotNull("the code sits on its own surface", card);
        assertEquals("@drawable/card_bg", attr(card, "background"));
        Element code = byId("activity_main.xml", "tCode");
        assertEquals("the digits are strong foreground text",
                "@color/jafnify_fg", attr(code, "textColor"));
        assertNull("the tracking comes from UiText, not the XML", attr(code, "letterSpacing"));
        Element cancel = byId("activity_main.xml", "bCancel");
        Element newCode = byId("activity_main.xml", "bNewCode");
        assertEquals("Cancel is the one filled action during pairing",
                "@style/JafnifyPrimaryButton", cancel.getAttribute("style"));
        assertEquals("New code is the quiet secondary action",
                "@style/JafnifyTextButton", newCode.getAttribute("style"));
    }

    // ---- accessibility -----------------------------------------------------

    @Test
    public void accessibilityLiveRegionAppearsExactlyTwice() {
        int count = 0;
        for (File layout : layouts()) {
            for (Element e : elements(layout)) {
                if (attr(e, "accessibilityLiveRegion") != null) {
                    count++;
                    assertEquals(layout.getName() + " is polite", "polite",
                            attr(e, "accessibilityLiveRegion"));
                }
            }
        }
        assertEquals("status and health are the only live regions", 2, count);
        Element status = byId("activity_main.xml", "tStatus");
        Element health = byId("activity_main.xml", "tHealth");
        assertEquals("polite", attr(status, "accessibilityLiveRegion"));
        assertEquals("polite", attr(health, "accessibilityLiveRegion"));
    }

    @Test
    public void iconOnlyControlsCarryAContentDescription() {
        Element more = byId("activity_main.xml", "bMore");
        assertNotNull("the overflow button exists", more);
        String description = attr(more, "contentDescription");
        assertNotNull("the overflow button has a description", description);
        assertTrue("the description comes from a string resource",
                description.startsWith("@string/"));
    }

    @Test
    public void theHomeHeaderCarriesTheWordmarkAndTheCardKeepsNoTitle() {
        // Since Batch F the app draws its own header: the J-Signal mark (the J
        // and the arcs are separate layers so the arcs can carry the state)
        // plus the wordmark. The card still carries no second title.
        Document doc = parse(new File(resDir(), "layout/activity_main.xml"));
        Element root = doc.getDocumentElement();
        boolean hasHeader = false;
        for (Element e : elements(new File(resDir(), "layout/activity_main.xml"))) {
            if ("include".equals(e.getTagName())
                    && "@layout/header_home".equals(e.getAttribute("layout"))) {
                hasHeader = true;
            }
        }
        assertTrue("the home screen includes the brand header", hasHeader);
        Element header = parse(new File(resDir(), "layout/header_home.xml"))
                .getDocumentElement();
        assertNotNull("the header draws the J", byIdIn(header, "headJ"));
        assertNotNull("the header arcs are a separate layer", byIdIn(header, "headArcs"));
        Element wordmark = byIdIn(header, "tWordmark");
        assertNotNull("the header carries the wordmark", wordmark);
        assertEquals("@string/wordmark", attr(wordmark, "text"));
        for (Element e : elements(new File(resDir(), "layout/activity_main.xml"))) {
            assertFalse("activity_main.xml has no tTitle view",
                    "@+id/tTitle".equals(attr(e, "id")));
            assertFalse("the app name appears once, in the header",
                    "@string/app_name".equals(attr(e, "text")));
        }
        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertFalse("MainActivity does not drive a second title", main.contains("R.id.tTitle"));
    }

    @Test
    public void textViewsWithACompoundDrawableDoNotAddASecondNode() {
        for (File layout : layouts()) {
            for (Element e : elements(layout)) {
                boolean drawable = attr(e, "drawableStart") != null
                        || attr(e, "drawableLeft") != null
                        || attr(e, "drawableTop") != null;
                if (drawable && !e.getTagName().equals("Button")) {
                    assertTrue(layout.getName() + " " + attr(e, "id")
                                    + " carries its meaning in text, not a description",
                            attr(e, "contentDescription") == null);
                }
            }
        }
    }

    // ---- 320 dp / 200 % ----------------------------------------------------

    @Test
    public void layoutsHaveNoFixedWidthThatCannotGrow() {
        for (File layout : layouts()) {
            for (Element e : elements(layout)) {
                String where = layout.getName() + " " + e.getTagName() + " "
                        + (attr(e, "id") == null ? "" : attr(e, "id"));
                String width = attr(e, "layout_width");
                if (width != null) {
                    assertTrue(where + " width is fluid: " + width,
                            width.equals("match_parent") || width.equals("wrap_content")
                                    || width.equals("0dp"));
                }
                String minWidth = attr(e, "minWidth");
                if (minWidth != null) {
                    assertEquals(where + " minWidth is only the 48dp target",
                            "48dp", minWidth);
                }
                assertTrue(where + " has no fixed dp width attribute",
                        attr(e, "width") == null);
            }
        }
    }

    @Test
    public void homeScrollsSoDetailsStayReachable() {
        Document doc = parse(new File(resDir(), "layout/activity_main.xml"));
        Element scroll = null;
        NodeList scrolls = doc.getElementsByTagName("ScrollView");
        if (scrolls.getLength() > 0) {
            scroll = (Element) scrolls.item(0);
        }
        assertNotNull("the home screen scrolls", scroll);
        assertEquals("fillViewport", "true", attr(scroll, "fillViewport"));
        assertEquals("match_parent", attr(scroll, "layout_width"));

        Element content = lastElementChild(scroll);
        assertNotNull("scroll content exists", content);
        assertEquals("wrap_content", attr(content, "layout_height"));
        assertNotNull("bottom padding keeps the last row off the edge",
                attr(content, "paddingBottom"));
        assertTrue("bottom padding uses a token",
                attr(content, "paddingBottom").startsWith("@dimen/jafnify_"));
        Element details = byIdIn(doc.getDocumentElement(), "bDetails");
        assertNotNull("the Details action is at the bottom", details);
        assertEquals("bDetails is a child of the scrolling container",
                content, details.getParentNode());

        Element panel = byIdIn(doc.getDocumentElement(), "detailsPanel");
        assertNotNull("the inline Details panel exists", panel);
        assertEquals("detailsPanel is a child of the scrolling container",
                content, panel.getParentNode());
        assertEquals("detailsPanel is the last child inside the scrolling container",
                panel, lastElementChild(content));
        assertEquals("detailsPanel starts collapsed", "gone", attr(panel, "visibility"));
        boolean beforePanel = false;
        for (Node node = content.getFirstChild(); node != null; node = node.getNextSibling()) {
            if (node == details) {
                beforePanel = true;
            }
            if (node == panel) {
                break;
            }
        }
        assertTrue("bDetails sits before its panel", beforePanel);

        // No ancestor may pin a fixed height: that is what clipped the link.
        for (Node node = panel.getParentNode(); node instanceof Element;
                node = node.getParentNode()) {
            Element ancestor = (Element) node;
            String height = attr(ancestor, "layout_height");
            String where = ancestor.getTagName() + " "
                    + (attr(ancestor, "id") == null ? "" : attr(ancestor, "id"));
            assertTrue(where + " height is fluid: " + height,
                    "match_parent".equals(height) || "wrap_content".equals(height)
                            || "0dp".equals(height));
            if ("0dp".equals(height)) {
                assertNotNull(where + " 0dp height is weighted", attr(ancestor, "layout_weight"));
            }
        }
    }

    @Test
    public void theNotificationWarningIsAnInlineBanner() {
        Element banner = byId("activity_settings.xml", "notifBanner");
        assertNotNull("the notification warning is a banner", banner);
        assertEquals("@drawable/banner_bg", attr(banner, "background"));
        Element note = byId("activity_settings.xml", "tNotifNote");
        assertEquals("the warning text uses the legal amber",
                "@color/jafnify_wait_text", attr(note, "textColor"));
        Element action = byId("activity_settings.xml", "bNotifSettings");
        assertNotNull("the banner has an action", action);
        assertEquals("@string/notif_open_settings", attr(action, "text"));
        assertNull("the action lines up under the message, not with a button indent",
                attr(action, "paddingStart"));
    }

    @Test
    public void theSecondaryScreensShareTheHeader() {
        for (String layout : new String[] {"activity_settings.xml", "activity_devices.xml",
                "activity_diagnostics.xml", "activity_about.xml",
                "activity_best_spot.xml", "activity_device_panel.xml"}) {
            boolean included = false;
            for (Element e : elements(new File(resDir(), "layout/" + layout))) {
                if ("include".equals(e.getTagName())
                        && "@layout/header_screen".equals(e.getAttribute("layout"))) {
                    included = true;
                }
            }
            assertTrue(layout + " includes the shared header", included);
        }
        String[][] screens = {
            {"SettingsActivity.java", "settings_title"},
            {"DevicesActivity.java", "devices_title"},
            {"DiagnosticsActivity.java", "diagnostics_title"},
            {"AboutActivity.java", "about_title"},
            {"BestSpotActivity.java", "best_spot_title"},
            {"DevicePanelActivity.java", "device_panel_title"},
        };
        for (String[] screen : screens) {
            String text = sourceOf(new File(javaDir(), screen[0]));
            assertTrue(screen[0] + " binds the shared header",
                    text.contains("ScreenHeader.bind(this,"));
            assertFalse(screen[0] + " does not also set the framework title",
                    text.contains("setTitle(R.string." + screen[1] + ")"));
        }
    }

    @Test
    public void theWideActionAndThemeRowsStackVertically() {
        Element actions = byId("activity_diagnostics.xml", "diagActions");
        assertNotNull("the diagnostics actions container exists", actions);
        assertEquals("the diagnostics actions stack vertically", "vertical",
                attr(actions, "orientation"));

        Element themes = byId("activity_settings.xml", "themeGroup");
        assertNotNull("the theme group exists", themes);
        assertEquals("the theme choices stack vertically", "vertical",
                attr(themes, "orientation"));
    }

    @Test
    public void everyActivityWrapsTheStoredTheme() {
        for (File source : javaDir().listFiles()) {
            if (!source.getName().endsWith(".java")) {
                continue;
            }
            String text = sourceOf(source);
            if (!text.contains("extends Activity")) {
                continue;
            }
            assertTrue(source.getName() + " wraps the theme in attachBaseContext",
                    text.contains("protected void attachBaseContext(Context base)"));
            assertTrue(source.getName() + " uses ThemeMode.wrap",
                    text.contains("ThemeMode.wrap(base,"));
        }
    }

    // ---- tokens and icon layers -------------------------------------------

    @Test
    public void layoutsUseTheGeneratedTokensInsteadOfLiterals() {
        String[] spacingAndType = {
                "layout_margin", "layout_marginTop", "layout_marginBottom",
                "layout_marginStart", "layout_marginEnd", "padding", "paddingTop",
                "paddingBottom", "paddingStart", "paddingEnd", "paddingLeft",
                "paddingRight", "drawablePadding", "textSize",
        };
        for (File layout : layouts()) {
            for (Element e : elements(layout)) {
                for (String name : spacingAndType) {
                    String value = attr(e, name);
                    if (value == null) {
                        continue;
                    }
                    assertTrue(layout.getName() + " " + e.getTagName() + " " + name
                                    + " uses a token: " + value,
                            value.startsWith("@dimen/jafnify_")
                                    || value.startsWith("@android:")
                                    || value.startsWith("?"));
                }
            }
        }
    }

    @Test
    public void theCardHasAGroundBorderAndElevation() {
        String card = sourceOf(new File(resDir(), "drawable/card_bg.xml"));
        assertTrue(card.contains("@color/jafnify_surface"));
        assertTrue(card.contains("@color/jafnify_line"));
        assertTrue(card.contains("@dimen/jafnify_radius_md"));
        Element status = byId("activity_main.xml", "statusCard");
        assertEquals("@dimen/jafnify_elevation_low", attr(status, "elevation"));
    }

    @Test
    public void iconVectorsUseTokenColoursAndAThemeBackground() {
        for (String name : new String[] {
                "ic_launcher_foreground.xml", "ic_launcher_background.xml",
                "ic_launcher_monochrome.xml", "ic_notification.xml"}) {
            String xml = sourceOf(new File(resDir(), "drawable/" + name));
            assertTrue(name + " has no hand-copied hex",
                    !xml.matches("(?s).*#[0-9A-Fa-f]{6,8}.*"));
            assertTrue(name + " uses a token colour", xml.contains("@color/jafnify_"));
        }
        String background = sourceOf(new File(resDir(), "drawable/ic_launcher_background.xml"));
        assertTrue("the launcher ground is the theme bg",
                background.contains("@color/jafnify_bg"));
    }

    @Test
    public void theAdaptiveIconsDeclareMonochromeLayers() {
        for (String name : new String[] {"ic_launcher.xml", "ic_launcher_round.xml"}) {
            Document doc = parse(new File(resDir(), "mipmap-anydpi-v26/" + name));
            NodeList mono = doc.getElementsByTagName("monochrome");
            assertEquals(name + " has one monochrome layer", 1, mono.getLength());
        }
    }

    @Test
    public void aPlainVectorBacksTheLauncherBeforeApi26() {
        for (String name : new String[] {"ic_launcher.xml", "ic_launcher_round.xml"}) {
            File fallback = new File(resDir(), "mipmap-anydpi/" + name);
            assertTrue(name + " has a plain vector fallback", fallback.exists());
        }
    }

    // ---- Java sources ------------------------------------------------------

    @Test
    public void diagnosticsOpensAtTheTop() {
        // The details and log views are selectable, hence focusable. Without a
        // focus holder above them the ScrollView scrolls to the first focused
        // child on open, hiding the details/History header. The scroller itself
        // takes focus first, so the screen opens at offset 0.
        Element scroll = byIdIn(parse(new File(resDir(), "layout/activity_diagnostics.xml"))
                .getDocumentElement(), "diagScroll");
        assertNotNull("the diagnostics scroller has an id", scroll);
        assertEquals("the scroller is focusable", "true", attr(scroll, "focusable"));
        assertEquals("the scroller is focusable in touch mode", "true",
                attr(scroll, "focusableInTouchMode"));
        assertEquals("the scroller takes focus before its children", "beforeDescendants",
                attr(scroll, "descendantFocusability"));
    }

    @Test
    public void everyActivityHandlesSystemBarsOnce() {
        String[] activities = {
            "MainActivity.java", "SettingsActivity.java", "DevicesActivity.java",
            "DiagnosticsActivity.java", "AboutActivity.java",
            "BestSpotActivity.java", "DevicePanelActivity.java",
        };
        for (String name : activities) {
            String source = sourceOf(new File(javaDir(), name));
            assertTrue(name + " applies the shared system-bar padding",
                    source.contains("SystemBars.apply(this)"));
            assertFalse(name + " does not grow its own inset handling",
                    source.contains("setOnApplyWindowInsetsListener"));
        }
        String helper = sourceOf(new File(javaDir(), "SystemBars.java"));
        assertTrue("one helper owns the inset listener",
                helper.contains("setOnApplyWindowInsetsListener"));
        assertTrue("the helper adds the measured action bar height",
                helper.contains("bar.getHeight()"));
        assertTrue("the helper re-applies the padding after the bar is measured",
                helper.contains("addOnLayoutChangeListener"));
        assertTrue("the helper does not add the status bar twice: the measured bar"
                + " height already includes it", helper.contains("Math.max("));
        assertTrue("without an action bar the helper pads by the system bars alone",
                helper.contains("bar == null"));
    }

    @Test
    public void theStatusLineLeadsTheCard() {
        // After the duplicate title went away the card opened with a tall row
        // holding only the overflow button. The overflow now shares the status
        // row, so the status line is the first thing in the card.
        Element main = parse(new File(resDir(), "layout/activity_main.xml"))
                .getDocumentElement();
        Element card = byIdIn(main, "statusCard");
        assertNotNull(card);
        Element status = byIdIn(main, "tStatus");
        Element more = byIdIn(main, "bMore");
        assertNotNull(status);
        assertNotNull(more);
        assertEquals("the status text centres in its row",
                "center_vertical", attr(status, "gravity"));
        assertEquals("font padding does not push the line down",
                "false", attr(status, "includeFontPadding"));
        Node row = status.getParentNode();
        assertTrue("the overflow sits in the status row", row == more.getParentNode());
        assertTrue("the status row is the card's first child",
                row.getPreviousSibling() == null || row.getPreviousSibling().getNodeType() != Node.ELEMENT_NODE);
        assertTrue("the status row is inside the card",
                row.getParentNode() == card);
    }

    @Test
    public void theTileHomeHasItsCardTilesAndButtons() {
        assertNotNull("the tile home has a block", byId("activity_main.xml", "tileHome"));
        assertNotNull("the tile home has a card", byId("activity_main.xml", "tileCard"));
        assertNotNull("the card has a title", byId("activity_main.xml", "tTileTitle"));
        assertNotNull("the card has a subline", byId("activity_main.xml", "tTileSub"));
        assertNotNull("the card has the notice of the link", byId("activity_main.xml", "tTileNotice"));
        assertNotNull("the card has the stop button", byId("activity_main.xml", "bTileStop"));
        assertNotNull("the tiles sit in a grid", byId("activity_main.xml", "tileGrid"));
        assertNotNull("the row has the best spot button", byId("activity_main.xml", "bTileBestSpot"));
        assertNotNull("the row has the phone button", byId("activity_main.xml", "bTilePhone"));
        assertNotNull("the row has the statistics button", byId("activity_main.xml", "bTileStats"));
        // The buttons are big enough for a finger.
        assertTrue(attr(byId("activity_main.xml", "bTileStop"), "minHeight").endsWith("48dp"));
        assertTrue(attr(byId("activity_main.xml", "bTileBestSpot"), "minHeight").endsWith("48dp"));
        assertTrue(attr(byId("activity_main.xml", "bTilePhone"), "minHeight").endsWith("48dp"));
        assertTrue(attr(byId("activity_main.xml", "bTileStats"), "minHeight").endsWith("48dp"));
    }

    @Test
    public void theTileHomeIsTheAppAndTheOldCardIsTheCrashFallback() {
        assertEquals("gone until the first render", "gone",
                attr(byId("activity_main.xml", "tileHome"), "visibility"));
        String source = sourceOf(new File(javaDir(), "MainActivity.java"));
        // No switch gates the home any more; only a broken tile home falls back.
        assertFalse(source.contains("tile_home"));
        assertTrue(source.contains("if (!tileHomeBroken)"));
        assertTrue("a long press opens the layout screen",
                source.contains("HomeLayoutActivity.class"));
        assertTrue(source.contains("setOnLongClickListener"));
    }

    @Test
    public void javaSourcesHaveNoPixelLiterals() {
        File[] sources = javaDir().listFiles();
        assertTrue("Java sources exist", sources != null && sources.length > 0);
        for (File source : sources) {
            if (!source.getName().endsWith(".java")) {
                continue;
            }
            String text = sourceOf(source);
            assertTrue(source.getName() + " has no setPadding with px literals",
                    !text.matches("(?s).*\\bsetPadding\\s*\\(\\s*-?\\d+.*"));
            assertTrue(source.getName() + " has no px suffix",
                    !text.matches("(?s).*\\b\\d+px\\b.*"));
        }
    }

    @Test
    public void javaSourcesDoNotUseTheDeprecatedGetColorCall() {
        for (File source : javaDir().listFiles()) {
            if (!source.getName().endsWith(".java")) {
                continue;
            }
            String text = sourceOf(source);
            assertTrue(source.getName() + " resolves colours through the theme",
                    !text.contains("getResources().getColor("));
        }
    }

    @Test
    public void theStatusGlyphIsSizedFromTheTextNotAIntrinsicVector() {
        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertTrue("the status glyph is sized in code",
                main.contains("UiText.statusGlyphSizePx("));
        assertTrue("the status glyph uses explicit bounds", main.contains(".setBounds("));
        assertTrue("no fixed-dp intrinsic error icon",
                !main.contains("setCompoundDrawablesRelativeWithIntrinsicBounds(\n"
                        + "                R.drawable.ic_error"));
    }

    @Test
    public void statusLinesAreTheCoreLinesNotJavaResources() {
        String core = sourceOf(locate("jafnify-core/src/snapshot.rs"));
        for (String line : new String[] {
                "\"Not sharing\"",
                "\"Searching for the other device\"",
                "\"Pairing\"",
                "\"Connecting\"",
                "\"Connected\"",
                "\"Waiting for the first handshake\"",
                "\"Not reachable\"",
                "\"Code expired\"",
        }) {
            assertTrue("the core owns the wording " + line, core.contains(line));
        }

        assertTrue("the dead status resource mapper is gone",
                !new File(javaDir(), "StatusRes.java").exists());
        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertTrue("the running status is the native core line",
                main.contains("statusText = UiText.displayLine(parsed);"));
        assertTrue("the neutral flag is a display fact over the core line",
                main.contains("boolean neutral = uiRunning && parsed.neutral;"));
        assertTrue("only the documented peer suffix is added",
                main.contains("UiText.withPeer(statusText, peer)"));
        assertTrue("the healthy case is not a second sentence",
                !main.contains("R.string.status_connected"));
        assertTrue(!main.contains("R.string.status_searching"));
        assertTrue(!main.contains("R.string.status_pairing"));
        assertTrue(!main.contains("R.string.status_connecting"));
        assertTrue(!main.contains("R.string.status_retrying"));
        assertTrue("the not-running card uses the core idle line",
                main.contains("Native.idleStatus()"));
        assertTrue("the health line is the core line while the link is current",
                main.contains("setText(tHealth, linkLine)"));
        assertTrue("the path line is dropped while waiting for the hotspot",
                main.contains("UiText.linkLine(healthLine, uiRunning, waiting)"));
        assertTrue("the code expiry is the core line",
                main.contains("setText(tCodeExpiry, expiry)"));

        String service = sourceOf(new File(javaDir(), "SharerService.java"));
        assertTrue("the notification keeps the native core line",
                service.contains("p.line"));
        assertTrue("the waiting sentence wins in the notification",
                service.contains("UiText.notificationText("));
        assertTrue(!service.contains("StatusRes"));
    }

    @Test
    public void thePairingCodeUsesLetterSpacingNotLiteralSpaces() {
        Element code = byId("activity_main.xml", "tCode");
        assertNotNull(code);
        assertNull("the tracking comes from the shared constant, not the XML",
                attr(code, "letterSpacing"));
        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertTrue("the code text is not spaced in Java", !main.contains("spacedCode("));
        assertTrue(main.contains("UiText.codeDisplay("));
        assertTrue("MainActivity applies the shared code tracking",
                main.contains("setLetterSpacing(UiText.CODE_TRACKING)"));
        String uiText = sourceOf(new File(javaDir(), "UiText.java"));
        assertTrue("one constant feeds the fit rule and the view",
                uiText.contains("CODE_TRACKING = 0.2f"));
    }

    @Test
    public void theStatisticsScreenHasThreeWindowsAndTheMenuIsGated() {
        assertNotNull("today window", byId("activity_statistics.xml", "statToday"));
        assertNotNull("week window", byId("activity_statistics.xml", "statWeek"));
        assertNotNull("month window", byId("activity_statistics.xml", "statMonth"));
        assertNotNull("summary text", byId("activity_statistics.xml", "statLines"));
        assertNotNull("network report text", byId("activity_statistics.xml", "statNetwork"));

        String manifest = sourceOf(locate("src/main/AndroidManifest.xml"));
        assertTrue("the manifest declares the screen",
                manifest.contains("android:name=\".StatisticsActivity\""));

        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertFalse("usage statistics is standard, not a gate",
                main.contains("usage_statistics"));
        assertTrue("the overflow opens the statistics screen",
                main.contains("StatisticsActivity.class"));
    }

    @Test
    public void theBestSpotSessionSurvivesTheConfigurationChange() {
        // A rotation must not lose the samples and the marks of a session that
        // is running: the screen puts the state into the bundle and reads it
        // back under the same key.
        String screen = sourceOf(new File(javaDir(), "BestSpotActivity.java"));
        assertTrue("the screen saves the session state",
                screen.contains("onSaveInstanceState"));
        String saved = firstArgument(screen, "putStringArray(");
        String restored = firstArgument(screen, "getStringArray(");
        assertNotNull("the session state goes into the bundle", saved);
        assertNotNull("the session state comes back from the bundle", restored);
        assertEquals("the saved state comes back under the same key",
                saved, restored);
    }

    @Test
    public void theBestSpotScreenEndsTheSessionWhenItIsOver() {
        String screen = sourceOf(new File(javaDir(), "BestSpotActivity.java"));
        assertTrue("the screen asks the core how long a session lasts",
                screen.contains("bestSpotSessionMs"));
        assertTrue("the screen ends the session when the length is up",
                screen.contains("expired("));
    }

    @Test
    public void theBestSpotSwitchCarriesTheSignalLevelAndTheStrength() {
        // The laptop's best-spot block reads the phone's summary, so the phone
        // must send the level and the strength while its best-spot switch is
        // on, even with the report switches off.
        String service = sourceOf(new File(javaDir(), "SharerService.java"));
        String bestSpot = assignedVariable(service, "getBoolean(\"best_spot\"");
        assertNotNull("the service reads the phone's best-spot switch", bestSpot);
        assertTrue("the signal gate opens for the best-spot switch",
                callArguments(service, "gatedSignal(").contains(bestSpot));
        assertTrue("the strength gate opens for the best-spot switch",
                callArguments(service, "gatedDbm(").contains(bestSpot));
    }

    @Test
    public void theBestSpotScreenHasItsButtonsItsLinesAndItsManifestEntry() {
        assertNotNull("start", byId("activity_best_spot.xml", "spotStart"));
        assertNotNull("stop", byId("activity_best_spot.xml", "spotStop"));
        assertNotNull("this spot", byId("activity_best_spot.xml", "spotMark"));
        assertNotNull("live line", byId("activity_best_spot.xml", "spotLive"));
        assertNotNull("spot list", byId("activity_best_spot.xml", "spotList"));
        assertNotNull("verdict", byId("activity_best_spot.xml", "spotVerdict"));

        String manifest = sourceOf(locate("src/main/AndroidManifest.xml"));
        assertTrue("the manifest declares the screen",
                manifest.contains("android:name=\".BestSpotActivity\""));
        Element activity = manifestActivity(".BestSpotActivity");
        assertNotNull("the best-spot screen is in the manifest", activity);
        assertEquals("the best-spot screen is not exported", "false",
                attr(activity, "exported"));

        String screen = sourceOf(new File(javaDir(), "BestSpotActivity.java"));
        assertTrue("the screen keeps the screen on while it is open",
                screen.contains("FLAG_KEEP_SCREEN_ON"));
        assertTrue("the samples live in memory only",
                screen.contains("Native.bestSpotLines("));
    }

    @Test
    public void theDevicePanelScreenHasItsLinesItsBatteryRowAndItsManifestEntry() {
        assertNotNull("panel lines", byId("activity_device_panel.xml", "panelLines"));
        assertNotNull("battery help row", byId("activity_device_panel.xml", "bBatteryHelp"));

        String manifest = sourceOf(locate("src/main/AndroidManifest.xml"));
        assertTrue("the manifest declares the screen",
                manifest.contains("android:name=\".DevicePanelActivity\""));
        Element activity = manifestActivity(".DevicePanelActivity");
        assertNotNull("the device panel is in the manifest", activity);
        assertEquals("the device panel is not exported", "false",
                attr(activity, "exported"));

        String screen = sourceOf(new File(javaDir(), "DevicePanelActivity.java"));
        assertTrue("the panel reads the core lines", screen.contains("Native.devicePanel("));
        assertTrue("the panel opens the existing battery help",
                screen.contains("SettingsActivity.class"));
        String facts = sourceOf(new File(javaDir(), "DeviceFacts.java"));
        assertTrue("the facts are collected on the phone", facts.contains("Build.MANUFACTURER"));
        assertTrue("the facts are collected on the phone", facts.contains("Build.MODEL"));
        assertTrue("no new permission is asked for", !facts.contains("Manifest.permission"));
    }

    @Test
    public void theOverflowOpensTheNewScreensOnlyWhenTheirSwitchIsOn() {
        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertTrue("the overflow reads the best-spot switch", main.contains("best_spot"));
        assertTrue("the overflow opens the best-spot screen",
                main.contains("BestSpotActivity.class"));
        assertTrue("the overflow reads the device panel switch", main.contains("device_panel"));
        assertTrue("the overflow opens the device panel screen",
                main.contains("DevicePanelActivity.class"));
    }

    @Test
    public void theNativeInstanceIsReadSafelyFromTheOtherThread() {
        // The service frees the instance on its own thread while the screen
        // reads the handle: the read has to see the latest value.
        String nativeSource = sourceOf(new File(javaDir(), "Native.java"));
        assertTrue("the instance handle is volatile",
                nativeSource.contains("volatile long instance"));
    }

    @Test
    public void theInstanceHandleIsZeroedBeforeTheNativeFreeAndEnsureIsSynchronized() {
        // F2: two threads must not create two instances, and destroy must
        // zero the handle before the native free so another thread never
        // passes the freed value.
        String src = sourceOf(new File(javaDir(), "Native.java"));
        assertTrue("ensure is synchronized",
                src.contains("public static synchronized long ensure("));
        assertTrue("destroy is synchronized",
                src.contains("public static synchronized void destroy()"));
        int destroyAt = src.indexOf("public static synchronized void destroy()");
        int zero = src.indexOf("instance = 0;", destroyAt);
        int free = src.indexOf("destroy(handle);", destroyAt);
        assertTrue("the handle is zeroed before the native free: " + src,
                zero >= 0 && free > zero);
        assertFalse("the raw free is not public",
                src.contains("public static native void destroy(long instance)"));
    }

    @Test
    public void theAboutScreenHasAThirdPartyNoticesRow() {
        // F4: the row's words are the core's, and it opens the generated asset.
        // The asset is 3.3 MB, so the screen loads it off the main thread
        // and shows small chunks in a list instead of one huge TextView.
        assertNotNull("the row exists", byId("activity_about.xml", "bNotices"));
        String about = sourceOf(new File(javaDir(), "AboutActivity.java"));
        assertTrue("the row text is the core's",
                about.contains("Native.thirdPartyNoticesTitle()"));
        assertTrue("the row opens the notices screen",
                about.contains("NoticesActivity.class"));
        String notices = sourceOf(new File(javaDir(), "NoticesActivity.java"));
        assertTrue("the notices screen reads the asset",
                notices.contains("THIRD_PARTY_NOTICES.txt"));
        Element list = byId("activity_notices.xml", "listNotices");
        assertNotNull("the notices screen is a list of small chunks", list);
        assertEquals("ListView", list.getTagName());
        assertNull("the one huge TextView is gone",
                byId("activity_notices.xml", "tNotices"));
        assertTrue("the screen splits the file into chunks",
                notices.contains("NoticesText.sections("));
        assertTrue("the read runs off the main thread",
                notices.contains("new Thread("));
        assertFalse("the whole file is never set in one view",
                notices.contains(".setText(read())"));
        String manifest = sourceOf(locate("src/main/AndroidManifest.xml"));
        assertTrue("the manifest declares the screen",
                manifest.contains("android:name=\".NoticesActivity\""));
        Element activity = manifestActivity(".NoticesActivity");
        assertNotNull("the notices screen is in the manifest", activity);
        assertEquals("the notices screen is not exported", "false",
                attr(activity, "exported"));
    }

    @Test
    public void theKeepScreenOnFlagOnlyRunsWhileSharingRuns() {
        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertTrue("MainActivity owns the flag", main.contains("FLAG_KEEP_SCREEN_ON"));
        assertTrue("the flag is added with addFlags",
                main.contains("addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)"));
        assertTrue("the flag is cleared again",
                main.contains("clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)"));
        assertTrue("the flag follows the switch", main.contains("keep_screen_on"));
        assertTrue("the flag needs a running share", main.contains("uiRunning"));
        assertTrue("the flag is set in one place", main.contains("applyKeepScreenOn("));
    }

    @Test
    public void theSharingSwitchAndPairDialogHaveTheirHelpText() {
        assertNotNull("start switch help", byId("activity_settings.xml", "tStartOnOpenHelp"));
        String strings = sourceOf(locate("src/main/res/values/strings.xml"));
        assertTrue("start switch help string",
                strings.contains("name=\"start_on_open_help\""));
        assertTrue("first pair dialog string",
                strings.contains("name=\"pair_new_none_message\""));
    }

    @Test
    public void theDeviceNameFieldIsLocalAndExplained() {
        assertNotNull("name field", byId("activity_settings.xml", "etNickname"));
        assertNotNull("name help", byId("activity_settings.xml", "tNicknameHelp"));
        String strings = sourceOf(locate("src/main/res/values/strings.xml"));
        assertTrue("name help string", strings.contains("name=\"nickname_help\""));
        String nativeSource = sourceOf(new File(javaDir(), "Native.java"));
        assertFalse("the nickname must not reach the native layer",
                nativeSource.contains("ickname"));
    }

    @Test
    public void the_keep_running_block_never_asks_for_a_battery_exemption() {
        String src = sourceOf(locate(
                "app/app/src/main/java/com/siliconstencil/jafnify/SettingsActivity.java"));
        assertTrue(src.contains("ACTION_AUTO_REVOKE_PERMISSIONS"));
        assertTrue(src.contains("ACTION_IGNORE_BACKGROUND_DATA_RESTRICTIONS_SETTINGS"));
        assertTrue(src.contains("isIgnoringBatteryOptimizations"));
        assertTrue(src.contains("isAutoRevokeWhitelisted"));
        assertTrue(src.contains("getRestrictBackgroundStatus"));
        assertFalse(src.contains("ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS"));
        assertFalse(src.contains("REQUEST_IGNORE_BATTERY_OPTIMIZATIONS"));
    }

    @Test
    public void theTileTokensAreTheOnesTheFitCheckAssumes() {
        // TileGridTest computes the room for "999.9 MB" from these three
        // numbers; pinning them here keeps the arithmetic tied to the theme.
        String dimens = sourceOf(new File(resDir(), "values/dimens.xml"));
        assertTrue(dimens.contains("name=\"jafnify_type_title\">22sp<"));
        assertTrue(dimens.contains("name=\"jafnify_spacing_md\">12dp<"));
        assertTrue(dimens.contains("name=\"jafnify_spacing_sm\">8dp<"));
    }

    @Test
    public void theGridFollowsTheSavedLayoutAndHidesAnEmptyRow() {
        // The saved layout owns the slot count and the wide slot; a 7 grid has
        // one full-width tile at the top or the bottom.
        String source = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertTrue(source.contains("homeLayout.slotCount()"));
        assertTrue(source.contains("homeLayout.wideSlot()"));
        assertTrue(source.contains("TileGrid.COLUMNS"));
        assertTrue(source.contains("setVisible(tileRows.get(row), any)"));
    }

    private static Element byId(String layout, String id) {
        for (Element e : elements(new File(resDir(), "layout/" + layout))) {
            if (("@" + "+id/" + id).equals(attr(e, "id"))) {
                return e;
            }
        }
        return null;
    }

    /** The manifest entry with this class name, or null. */
    private static Element manifestActivity(String name) {
        Document doc = parse(locate("src/main/AndroidManifest.xml"));
        NodeList activities = doc.getElementsByTagName("activity");
        for (int i = 0; i < activities.getLength(); i++) {
            Element activity = (Element) activities.item(i);
            if (name.equals(attr(activity, "name"))) {
                return activity;
            }
        }
        return null;
    }

    /** A tone colour is already a colour int; resolving it again as a resource id crashed the screen. */
    @org.junit.Test
    public void theTileHomeNeverResolvesAColourIntAsAResource() throws Exception {
        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        org.junit.Assert.assertFalse(main.contains("Palette.colour(this, toneColour("));
        org.junit.Assert.assertTrue("a failing tile home falls back to the old card",
                main.contains("tileHomeBroken = true;"));
    }

    /** A plain View with wrap_content takes all the room it is given: the dot filled the card. */
    @org.junit.Test
    public void theTileDotHasAFixedSize() {
        assertEquals("ImageView", byId("activity_main.xml", "tileDot").getTagName());
        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertTrue(main.contains("setImageDrawable(dot)"));
        assertFalse(main.contains("tileDot.setBackground("));
    }

    /** Every tile the same size, the grid even. */
    @org.junit.Test
    public void theTilesOfARowShareOneHeightAndKeepTheirFactRoom() {
        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertTrue(main.contains("0, LinearLayout.LayoutParams.MATCH_PARENT, 1f)"));
        assertTrue(main.contains("keepFact(views, views.factOne"));
        assertTrue(main.contains("keepFact(views, views.factTwo"));
        assertTrue(main.contains("view.setVisibility(View.INVISIBLE)"));
        assertTrue(main.contains("setSingleLine(true)"));
    }

    /** The three buttons under the tiles are equal boxes. */
    @org.junit.Test
    public void theTileButtonsAreEqualBoxes() {
        for (String id : new String[] {"bTileBestSpot", "bTilePhone", "bTileStats"}) {
            Element e = byId("activity_main.xml", id);
            assertEquals(id, "0dp", attr(e, "layout_width"));
            assertEquals(id, "1", attr(e, "layout_weight"));
            assertEquals(id, "@drawable/card_bg", attr(e, "background"));
        }
    }

    /** The sharing home is one screen, so the frame around it is thinner. */
    @Test
    public void theSharingHomePullsTheTextInAndShrinksTheCard() {
        Element root = parse(new File(resDir(), "layout/activity_main.xml"))
                .getDocumentElement();
        assertEquals("the root padding pulls the wordmark in",
                "@dimen/jafnify_spacing_sm", attr(root, "padding"));
        Element header = parse(new File(resDir(), "layout/header_home.xml"))
                .getDocumentElement();
        assertEquals("the header sits closer to the card",
                "@dimen/jafnify_spacing_xs", attr(header, "paddingBottom"));
        Element card = byId("activity_main.xml", "tileCard");
        assertEquals("the card padding is tighter",
                "@dimen/jafnify_spacing_md", attr(card, "padding"));
        assertEquals("the card title is one step smaller than the title size",
                "@dimen/jafnify_type_body_large",
                attr(byId("activity_main.xml", "tTileTitle"), "textSize"));
        assertEquals("the Stop row keeps only the small gap",
                "@dimen/jafnify_spacing_sm",
                attr(byId("activity_main.xml", "tileCardButtons"), "layout_marginTop"));

        Element scroll = null;
        NodeList scrolls = parse(new File(resDir(), "layout/activity_main.xml"))
                .getElementsByTagName("ScrollView");
        if (scrolls.getLength() > 0) {
            scroll = (Element) scrolls.item(0);
        }
        assertNotNull(scroll);
        Element content = lastElementChild(scroll);
        assertEquals("the last row sits closer to the screen edge",
                "@dimen/jafnify_spacing_sm", attr(content, "paddingBottom"));
    }

    /** The tiles lose height, not readable text. */
    @Test
    public void theCompactTilesShaveHeightButKeepTheFactSize() {
        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertTrue("the tile text drops the extra font padding",
                main.contains("setIncludeFontPadding(false)"));
        assertTrue("a compact tile pads less above and below",
                main.contains("int padY = compact ? dimen(R.dimen.jafnify_spacing_xs) : pad;"));
        assertTrue("a compact tile draws its trend on the caption line",
                main.contains("dimen(compact ? R.dimen.jafnify_line_height_caption"
                        + "\n                        : R.dimen.jafnify_line_height_title)"));
        assertTrue("the fact text keeps the caption size",
                main.contains("view.setTextSize(TypedValue.COMPLEX_UNIT_PX,\n"
                        + "                getResources().getDimension(R.dimen.jafnify_type_caption));"));
        assertTrue("the number keeps the title size",
                main.contains("views.value.setTextSize(TypedValue.COMPLEX_UNIT_PX,\n"
                        + "                getResources().getDimension(R.dimen.jafnify_type_title));"));
    }

    /** The grid opens with the small gap, so 8 tiles fit above the buttons. */
    @Test
    public void theGridOpensWithTheSmallGap() {
        String main = sourceOf(new File(javaDir(), "MainActivity.java"));
        assertTrue("the compact grid top margin is the small gap",
                main.contains("int top = dimen(compact ? R.dimen.jafnify_spacing_sm"
                        + "\n                : R.dimen.jafnify_spacing_lg);"));
    }
}
