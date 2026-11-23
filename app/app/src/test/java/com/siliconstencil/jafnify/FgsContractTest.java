package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
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
 * Contracts for the stability batch that must not regress silently: the
 * foreground-service type, the update receiver, the Wi-Fi lock and the
 * battery hint. Pure JVM: it reads the manifest and the sources as text.
 */
public class FgsContractTest {
    private static final String ANDROID = "http://schemas.android.com/apk/res/android";

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

    private static File java(String name) {
        return locate("src/main/java/com/siliconstencil/jafnify/" + name);
    }

    private static Document manifest() {
        try {
            DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
            factory.setNamespaceAware(true);
            return factory.newDocumentBuilder().parse(locate("src/main/AndroidManifest.xml"));
        } catch (Exception e) {
            throw new AssertionError("cannot parse the manifest", e);
        }
    }

    private static String source(File file) {
        try {
            return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
        } catch (IOException e) {
            throw new AssertionError("cannot read " + file, e);
        }
    }

    private static List<Element> elements(Element root) {
        List<Element> out = new ArrayList<>();
        collect(root, out);
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

    private static String name(Element e) {
        String value = e.getAttributeNS(ANDROID, "name");
        return value == null ? "" : value;
    }

    private static String attr(Element e, String key) {
        String value = e.getAttributeNS(ANDROID, key);
        return value == null || value.isEmpty() ? null : value;
    }

    // ---- 1. foreground-service type ----------------------------------------

    @Test
    public void theSharerIsConnectedDeviceAndNeverDataSync() {
        Element service = null;
        for (Element e : elements(manifest().getDocumentElement())) {
            if ("service".equals(e.getTagName()) && ".SharerService".equals(name(e))) {
                service = e;
            }
        }
        assertNotNull("the sharer service is declared", service);
        String type = attr(service, "foregroundServiceType");
        assertNotNull("the service declares a type", type);
        assertTrue("the documented fit is connectedDevice: " + type,
                type.contains("connectedDevice"));
        assertFalse("dataSync would add the Android 15 6 h cap: " + type,
                type.contains("dataSync"));
        assertFalse("specialUse is no longer needed: " + type,
                type.contains("specialUse"));

        boolean permission = false;
        for (Element e : elements(manifest().getDocumentElement())) {
            if ("uses-permission".equals(e.getTagName())
                    && "android.permission.FOREGROUND_SERVICE_CONNECTED_DEVICE"
                            .equals(name(e))) {
                permission = true;
            }
        }
        assertTrue("connectedDevice needs its permission on API 34+", permission);

        String java = source(java("SharerService.java"));
        assertTrue("startForeground passes the matching type",
                java.contains("FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE"));
        assertFalse("the old type is gone",
                java.contains("FOREGROUND_SERVICE_TYPE_SPECIAL_USE"));
        assertFalse("dataSync is never picked",
                java.contains("FOREGROUND_SERVICE_TYPE_DATA_SYNC"));
    }

    // ---- 2. an app update restarts a live session --------------------------

    @Test
    public void theUpdateReceiverIsDeclaredAndObeysTheStartPolicy() {
        Element receiver = null;
        for (Element e : elements(manifest().getDocumentElement())) {
            if ("receiver".equals(e.getTagName()) && ".UpdateReceiver".equals(name(e))) {
                receiver = e;
            }
        }
        assertNotNull("the update receiver is declared", receiver);
        assertEquals("it only takes the package's own broadcast", "false",
                attr(receiver, "exported"));
        boolean action = false;
        for (Element e : elements(receiver)) {
            if ("action".equals(e.getTagName())
                    && "android.intent.action.MY_PACKAGE_REPLACED".equals(name(e))) {
                action = true;
            }
        }
        assertTrue("it listens for MY_PACKAGE_REPLACED", action);

        String java = source(java("UpdateReceiver.java"));
        assertTrue("the saved live state is the gate",
                java.contains("KEY_SHARING_LIVE"));
        assertTrue("a user Stop still wins", java.contains("KEY_USER_STOPPED"));
        assertTrue("the one start decision is reused",
                java.contains("StartPolicy.Trigger.UPDATE"));
        assertTrue("MY_PACKAGE_REPLACED is an allowed FGS start",
                java.contains("startForegroundService"));
    }

    // ---- 3. the Wi-Fi lock follows the session -----------------------------

    @Test
    public void theWifiLockIsHeldOnlyWhileTheLinkIsServed() {
        String java = source(java("SharerService.java"));
        assertTrue("the lock is taken with the API-appropriate mode",
                java.contains("createWifiLock(RadioLock.mode("));
        assertTrue("the lock is released on the stop paths",
                java.contains("private void releaseRadioLock()"));
        assertTrue("waiting for the hotspot releases it",
                java.contains("releaseRadioLock();"));
        assertTrue("a restore re-arms it", java.contains("ensureRadioLock();"));
        assertTrue("the diag stat reports it", java.contains("stats.put(\"wifilock\""));
        assertFalse("it is not reported as always off",
                java.contains("stats.put(\"wifilock\", \"no\")"));
    }

    // ---- 4. the battery hint -----------------------------------------------

    @Test
    public void theHomeShowsTheBatteryHintAndOpensTheExemptionList() {
        Document layout = null;
        try {
            DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
            factory.setNamespaceAware(true);
            layout = factory.newDocumentBuilder()
                    .parse(locate("src/main/res/layout/activity_main.xml"));
        } catch (Exception e) {
            throw new AssertionError("cannot parse activity_main.xml", e);
        }
        boolean banner = false;
        boolean note = false;
        boolean dismiss = false;
        for (Element e : elements(layout.getDocumentElement())) {
            String id = attr(e, "id");
            if (id == null) {
                continue;
            }
            banner = banner || "@+id/batteryBanner".equals(id);
            note = note || "@+id/tBatteryNote".equals(id);
            dismiss = dismiss || "@+id/bBatteryDismiss".equals(id);
        }
        assertTrue("the card has the battery banner", banner);
        assertNotNull("the banner has its sentence", note);
        assertTrue("the banner is dismissible", dismiss);

        String main = source(java("MainActivity.java"));
        assertTrue("the pure rule owns the decision", main.contains("BatteryHint.show("));
        assertTrue("the tap opens the system exemption list",
                main.contains("ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS"));
        assertFalse("the app never asks for the exemption directly",
                main.contains("ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS"));
    }
}
