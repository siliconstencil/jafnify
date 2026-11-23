package com.siliconstencil.jafnify;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Build;

import java.lang.reflect.Method;
import java.net.Inet4Address;
import java.net.InterfaceAddress;
import java.net.NetworkInterface;
import java.util.ArrayList;
import java.util.Enumeration;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * The interfaces the sharer's discovery runs on.
 *
 * A phone acting as hotspot has two kinds of networks: mobile data (the
 * default network) and the hotspot LAN the laptop is on. Unpinned multicast
 * follows the default network, so announcements never reach the laptop. The
 * fix is to join the discovery group and announce on every up,
 * multicast-capable, non-loopback IPv4 interface except the cellular ones.
 *
 * Interface names differ per vendor (swlan0, ap0, wlan1...), so none is
 * matched. Cellular is identified through ConnectivityManager. A stacked
 * link of a cellular network (the 464XLAT CLAT is one) is excluded too, and
 * two address ranges that can never be a hotspot LAN are dropped even when
 * their interface is not recognised: 192.0.0.0/24 (IETF special, CLAT uses
 * 192.0.0.4) and 169.254.0.0/16 (link-local). Native
 * getifaddrs/netlink enumeration is restricted for apps targeting API 30+,
 * which is why this runs in Java and hands the list to native code.
 */
final class NetIfaces {
    private NetIfaces() {}

    /** One interface as plain values, so the rule below runs on the JVM. */
    static final class Candidate {
        final String name;
        final boolean up;
        final boolean multicast;
        final boolean loopback;
        final List<String> ipv4;
        /** Prefix length of each address in {@code ipv4}, same order. */
        final List<Integer> prefixes;
        /** A stacked link of a cellular network (the CLAT on most carriers). */
        final boolean stacked;

        Candidate(String name, boolean up, boolean multicast, boolean loopback,
                List<String> ipv4, List<Integer> prefixes, boolean stacked) {
            this.name = name;
            this.up = up;
            this.multicast = multicast;
            this.loopback = loopback;
            this.ipv4 = ipv4;
            this.prefixes = prefixes;
            this.stacked = stacked;
        }

        Candidate(String name, boolean up, boolean multicast, boolean loopback,
                List<String> ipv4, List<Integer> prefixes) {
            this(name, up, multicast, loopback, ipv4, prefixes, false);
        }

        /** Without prefixes (only the address list matters): /24 each. */
        Candidate(String name, boolean up, boolean multicast, boolean loopback,
                List<String> ipv4) {
            this(name, up, multicast, loopback, ipv4, defaultPrefixes(ipv4.size()), false);
        }

        Candidate(String name, boolean up, boolean multicast, boolean loopback,
                List<String> ipv4, boolean stacked) {
            this(name, up, multicast, loopback, ipv4, defaultPrefixes(ipv4.size()),
                    stacked);
        }

        private static List<Integer> defaultPrefixes(int n) {
            List<Integer> p = new ArrayList<>();
            for (int i = 0; i < n; i++) {
                p.add(24);
            }
            return p;
        }

        boolean usable(Set<String> cellular) {
            return up && multicast && !loopback && !stacked && !cellular.contains(name);
        }
    }

    /**
     * A hotspot LAN address. 192.0.0.0/24 is reserved for protocol
     * assignments (CLAT uses 192.0.0.4) and 169.254.0.0/16 is link-local;
     * neither can be the LAN the laptop connects to.
     */
    static boolean hotspotAddress(String address) {
        if (address == null || address.isEmpty()) {
            return false;
        }
        return !address.startsWith("192.0.0.") && !address.startsWith("169.254.");
    }

    /** The address class for diagnostics; never the raw address. */
    static String addressClass(String address) {
        if (address == null || address.isEmpty()) {
            return "other";
        }
        if (address.startsWith("192.0.0.")) {
            return "clat";
        }
        if (address.startsWith("169.254.")) {
            return "linklocal";
        }
        if (address.startsWith("10.") || address.startsWith("192.168.")) {
            return "lan";
        }
        if (address.startsWith("172.")) {
            String[] parts = address.split("\\.");
            if (parts.length > 1) {
                try {
                    int second = Integer.parseInt(parts[1]);
                    if (second >= 16 && second <= 31) {
                        return "lan";
                    }
                } catch (NumberFormatException ignored) {
                    // Not a dotted-quad we understand.
                }
            }
        }
        return "other";
    }

    /**
     * IPv4 addresses of the interfaces discovery should use, in enumeration
     * order: up, multicast-capable, not loopback, not in {@code cellular}.
     */
    static List<String> select(List<Candidate> all, Set<String> cellular) {
        List<String> out = new ArrayList<>();
        for (Candidate c : all) {
            if (!c.usable(cellular)) {
                continue;
            }
            for (String address : c.ipv4) {
                if (hotspotAddress(address)) {
                    out.add(address);
                }
            }
        }
        return out;
    }

    /**
     * The same selection as "name=address/prefix,..." for the probe, whose
     * report names the interface a frame came from.
     */
    static String selectLabelled(List<Candidate> all, Set<String> cellular) {
        StringBuilder sb = new StringBuilder();
        for (Candidate c : all) {
            if (!c.usable(cellular)) {
                continue;
            }
            for (int i = 0; i < c.ipv4.size(); i++) {
                if (!hotspotAddress(c.ipv4.get(i))) {
                    continue;
                }
                if (sb.length() > 0) {
                    sb.append(',');
                }
                sb.append(c.name).append('=').append(c.ipv4.get(i))
                        .append('/').append(c.prefixes.get(i));
            }
        }
        return sb.toString();
    }

    /** What to pass to {@code Native.probeStart}; "" when nothing qualifies. */
    static String discoverLabelled(Context context) {
        try {
            Set<String> cellular = cellularNames(context);
            return selectLabelled(candidates(context, cellular), cellular);
        } catch (Exception e) {
            return "";
        }
    }

    /** The list as native code takes it: "10.20.89.13,192.168.49.1". */
    static String join(List<String> addresses) {
        StringBuilder sb = new StringBuilder();
        for (String a : addresses) {
            if (sb.length() > 0) {
                sb.append(',');
            }
            sb.append(a);
        }
        return sb.toString();
    }

    /**
     * What to pass to {@code Native.sharerStart*} right now. "" (no usable
     * interface, or the platform refused to say) makes native code keep the
     * default route, which is no worse than before.
     */
    static String discover(Context context) {
        try {
            Set<String> cellular = cellularNames(context);
            return join(select(candidates(context, cellular), cellular));
        } catch (Exception e) {
            return "";
        }
    }

    /**
     * The full candidate list for the diagnostic journal, on every change of
     * the usable set: name, up, multicast, loopback, cellular, stacked and the
     * address class. No raw address and no MAC, so it is safe to export.
     * "" when the platform refuses to enumerate.
     */
    static String describe(Context context) {
        try {
            Set<String> cellular = cellularNames(context);
            return describe(candidates(context, cellular), cellular);
        } catch (Exception e) {
            return "";
        }
    }

    static String describe(List<Candidate> all, Set<String> cellular) {
        StringBuilder sb = new StringBuilder();
        for (Candidate c : all) {
            if (sb.length() > 0) {
                sb.append(';');
            }
            sb.append("name=").append(c.name)
                    .append(",up=").append(bit(c.up))
                    .append(",mcast=").append(bit(c.multicast))
                    .append(",loop=").append(bit(c.loopback))
                    .append(",cell=").append(bit(cellular.contains(c.name)))
                    .append(",stacked=").append(bit(c.stacked))
                    .append(",addr=");
            if (c.ipv4.isEmpty()) {
                sb.append("none");
            } else {
                for (int i = 0; i < c.ipv4.size(); i++) {
                    if (i > 0) {
                        sb.append('+');
                    }
                    sb.append(addressClass(c.ipv4.get(i)));
                }
            }
        }
        return sb.toString();
    }

    private static String bit(boolean value) {
        return value ? "1" : "0";
    }

    /**
     * True when an active network exists, even before it validates. A captive
     * portal or an unvalidated Wi-Fi is still an uplink; only no uplink at all
     * is "No network".
     */
    static boolean hasUplink(Context context) {
        if (Build.VERSION.SDK_INT < 23) {
            return true;
        }
        ConnectivityManager cm = (ConnectivityManager) context
                .getSystemService(Context.CONNECTIVITY_SERVICE);
        if (cm == null) {
            return true;
        }
        Network active = cm.getActiveNetwork();
        if (active == null) {
            return false;
        }
        NetworkCapabilities caps = cm.getNetworkCapabilities(active);
        if (caps == null) {
            return true;
        }
        return caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                || caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
                || caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)
                || caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)
                || caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN);
    }

    private static List<Candidate> candidates(Context context, Set<String> cellular)
            throws Exception {
        Set<String> stacked = stackedNames(context);
        List<Candidate> out = new ArrayList<>();
        Enumeration<NetworkInterface> all = NetworkInterface.getNetworkInterfaces();
        while (all != null && all.hasMoreElements()) {
            NetworkInterface ni = all.nextElement();
            NetworkInterface parent = ni.getParent();
            boolean isStacked = stacked.contains(ni.getName())
                    || (parent != null && cellular.contains(parent.getName()));
            List<String> v4 = new ArrayList<>();
            List<Integer> prefixes = new ArrayList<>();
            for (InterfaceAddress ia : ni.getInterfaceAddresses()) {
                if (ia.getAddress() instanceof Inet4Address) {
                    v4.add(ia.getAddress().getHostAddress());
                    prefixes.add((int) ia.getNetworkPrefixLength());
                }
            }
            out.add(new Candidate(ni.getName(), ni.isUp(), ni.supportsMulticast(),
                    ni.isLoopback(), v4, prefixes, isStacked));
        }
        return out;
    }

    /**
     * Interface names of the links stacked under a cellular network: the CLAT
     * is one, and LinkProperties.getInterfaceName() of the cellular network
     * does not name it. {@code getStackedLinks} is a system API that is not in
     * the public SDK, so it is called reflectively; the platform may refuse,
     * and the address rules and the interface parent below still apply.
     * Needs API 29; older phones simply have no set.
     */
    private static Set<String> stackedNames(Context context) {
        Set<String> names = new HashSet<>();
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return names;
        }
        ConnectivityManager cm = (ConnectivityManager) context
                .getSystemService(Context.CONNECTIVITY_SERVICE);
        if (cm == null) {
            return names;
        }
        Method stackedLinks = null;
        try {
            stackedLinks = LinkProperties.class.getMethod("getStackedLinks");
        } catch (NoSuchMethodException e) {
            return names;
        }
        for (Network n : cm.getAllNetworks()) {
            NetworkCapabilities caps = cm.getNetworkCapabilities(n);
            LinkProperties lp = cm.getLinkProperties(n);
            if (caps == null || lp == null
                    || !caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)) {
                continue;
            }
            Object links;
            try {
                links = stackedLinks.invoke(lp);
            } catch (Exception | LinkageError e) {
                continue;
            }
            if (!(links instanceof Iterable)) {
                continue;
            }
            for (Object link : (Iterable<?>) links) {
                if (link instanceof LinkProperties) {
                    String name = ((LinkProperties) link).getInterfaceName();
                    if (name != null) {
                        names.add(name);
                    }
                }
            }
        }
        return names;
    }

    /** Interface names of the networks whose transport is cellular. */
    @SuppressWarnings("deprecation")
    private static Set<String> cellularNames(Context context) {
        Set<String> names = new HashSet<>();
        ConnectivityManager cm = (ConnectivityManager) context
                .getSystemService(Context.CONNECTIVITY_SERVICE);
        if (cm == null) {
            return names;
        }
        for (Network n : cm.getAllNetworks()) {
            NetworkCapabilities caps = cm.getNetworkCapabilities(n);
            LinkProperties lp = cm.getLinkProperties(n);
            if (caps != null && lp != null && lp.getInterfaceName() != null
                    && caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)) {
                names.add(lp.getInterfaceName());
            }
        }
        return names;
    }
}
