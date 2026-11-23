package com.siliconstencil.jafnify;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Locale;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * Product rule: Jafnify never talks about hiding - it neither promises nor
 * denies that traffic, tethering or hotspot use is hidden, and it does not say
 * what a mobile network can see. Users who install it know what it is. Only the
 * neutral plan-terms reminder ("your mobile plan's terms") remains.
 */
public class WordingTest {
    private static final String[] BANNED = {
        "undetectable", "invisible", "anonymous", "anonymity", "stealth",
        "bypass", "free internet", "unlimited",
    };

    private static final Pattern HIDING = Pattern.compile(
            "\\bcarrier\\b"
            + "|\\boperator\\s+(sees|can see|cannot see)\\b"
            + "|\\b(hide|hides|hiding|hidden)\\b[^.]{0,60}"
            + "\\b(traffic|tether\\w*|usage|hotspot)\\b"
            + "|\\b(traffic|tether\\w*|usage|hotspot)\\b[^.]{0,60}"
            + "\\b(hide|hides|hiding|hidden)\\b",
            Pattern.CASE_INSENSITIVE);

    private static final String[] USER_FACING = {
        "app/app/src/main/res/values/strings.xml",
        "linux/src/view.rs",
        "linux/src/ui_text.rs",
        "jafnify-core/src/flags.rs",
        "jafnify-core/src/snapshot.rs",
        "jafnify-core/src/terms.rs",
    };

    private static final String[] DOCS = {
        "README.md",
        "SECURITY.md",
        "CHANGELOG.md",
        "CONTRIBUTING.md",
        "docs/ARCHITECTURE.md",
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

    private static String read(File file) {
        try {
            return new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
        } catch (IOException e) {
            throw new AssertionError("cannot read " + file, e);
        }
    }

    private static void assertNoHiding(String where, String text) {
        Matcher m = HIDING.matcher(text);
        assertFalse(where + " must not talk about hiding or network visibility",
                m.find());
    }

    private static void assertNoBannedWord(String where, String text) {
        String lower = text.toLowerCase(Locale.ROOT);
        for (String word : BANNED) {
            assertFalse(where + " must not contain '" + word + "'",
                    lower.contains(word));
        }
    }

    @Test
    public void userVisibleTextNeverTalksAboutHidingOrTheCarrier() {
        for (String relative : USER_FACING) {
            String text = read(locate(relative));
            assertNoHiding(relative, text);
            assertNoBannedWord(relative, text);
        }
    }

    @Test
    public void theDocsNeverTalkAboutHidingOrTheCarrier() {
        assertTrue("the public docs were found", DOCS.length >= 4);
        for (String relative : DOCS) {
            assertNoHiding(relative, read(locate(relative)));
        }
    }

    @Test
    public void theTermsKeepTheNeutralPlanReminder() {
        String core = read(locate("jafnify-core/src/terms.rs"));
        String xml = read(locate("app/app/src/main/res/values/strings.xml"));
        assertTrue("the core terms mention the mobile plan's terms",
                core.contains("mobile plan's terms"));
        assertTrue("the Android terms mention the mobile plan's terms",
                xml.contains("mobile plan\\'s terms") || xml.contains("mobile plan's terms"));
        assertTrue("the core terms still say what the link does",
                core.contains("Jafnify encrypts the link between your computer and your"));
    }
}
