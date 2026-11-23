package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.w3c.dom.NodeList;

import java.io.File;
import java.util.ArrayList;
import java.util.List;

import javax.xml.parsers.DocumentBuilderFactory;

/**
 * The J-Signal mark: one SVG source (J plus two signal arcs), copied verbatim
 * into every Android icon. Pure JVM, like {@link LayoutContractTest}.
 */
public class MarkTest {
    private static final String ANDROID = "http://schemas.android.com/apk/res/android";
    private static final String[] ICONS = {
        "drawable/ic_launcher_foreground.xml",
        "drawable/ic_launcher_monochrome.xml",
        "drawable/ic_notification.xml",
        "mipmap-anydpi/ic_launcher.xml",
        "mipmap-anydpi/ic_launcher_round.xml",
    };

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
        throw new AssertionError("not found: " + relative);
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

    private static String normalise(String data) {
        return data.replace(",", " ").replaceAll("\\s+", " ").trim();
    }

    private static List<Element> paths(File file) {
        NodeList nodes = parse(file).getElementsByTagName("path");
        List<Element> out = new ArrayList<>();
        for (int i = 0; i < nodes.getLength(); i++) {
            Node node = nodes.item(i);
            if (node instanceof Element) {
                out.add((Element) node);
            }
        }
        return out;
    }

    private static List<String> svgMarkPaths() {
        List<String> out = new ArrayList<>();
        for (Element p : paths(locate("assets/icon/jafnify.svg"))) {
            String d = p.getAttribute("d");
            if (d != null && !d.isEmpty()) {
                out.add(normalise(d));
            }
        }
        return out;
    }

    @Test
    public void theSvgHoldsTheJAndTwoSignalArcs() {
        File svg = locate("assets/icon/jafnify.svg");
        List<Element> paths = paths(svg);
        assertEquals("the mark is the J plus two arcs", 3, paths.size());

        Element j = paths.get(0);
        assertEquals("the J is stroked, not filled", "7", j.getAttribute("stroke-width"));
        assertEquals("round caps", "round", j.getAttribute("stroke-linecap"));
        assertTrue("the J starts at its top end", normalise(j.getAttribute("d"))
                .startsWith("M40 16"));

        for (int i = 1; i <= 2; i++) {
            Element arc = paths.get(i);
            assertEquals("arc " + i + " stroke", "5.5", arc.getAttribute("stroke-width"));
            assertEquals("arc " + i + " is ember", "#FF5C6C", arc.getAttribute("stroke"));
            assertTrue("arc " + i + " is an SVG arc command",
                    normalise(arc.getAttribute("d")).contains(" A10 10 ")
                            || normalise(arc.getAttribute("d")).contains(" A17 17 "));
        }
    }

    @Test
    public void everyAndroidIconCarriesAllThreeSvgPaths() {
        List<String> mark = svgMarkPaths();
        assertEquals(3, mark.size());
        for (String icon : ICONS) {
            List<String> found = new ArrayList<>();
            for (Element p : paths(locate("app/app/src/main/res/" + icon))) {
                String data = p.getAttributeNS(ANDROID, "pathData");
                found.add(normalise(data == null || data.isEmpty()
                        ? p.getAttribute("pathData") : data));
            }
            for (String d : mark) {
                assertTrue(icon + " is missing the path " + d, found.contains(d));
            }
        }
    }

    @Test
    public void theAdaptiveForegroundUsesTheTokenColours() {
        String xml = new String(read(locate(
                "app/app/src/main/res/drawable/ic_launcher_foreground.xml")));
        assertTrue("the J is the fg token", xml.contains("android:strokeColor=\"@color/jafnify_fg\""));
        assertTrue("the arcs are the accent token",
                xml.contains("android:strokeColor=\"@color/jafnify_accent\""));
        assertNotNull("the foreground is readable", xml);
    }

    private static byte[] read(File file) {
        try {
            return java.nio.file.Files.readAllBytes(file.toPath());
        } catch (Exception e) {
            throw new AssertionError("cannot read " + file, e);
        }
    }
}
