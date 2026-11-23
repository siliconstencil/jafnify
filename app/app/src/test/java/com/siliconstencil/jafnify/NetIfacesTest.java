package com.siliconstencil.jafnify;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * Which interfaces discovery runs on. Every up, multicast-capable,
 * non-loopback IPv4 interface except the
 * cellular ones. Cellular is decided by the caller from ConnectivityManager,
 * never from an interface name: vendors call the hotspot swlan0, ap0 or wlan1.
 */
public class NetIfacesTest {
    private static NetIfaces.Candidate nic(String name, boolean up, boolean multicast,
            boolean loopback, String... ipv4) {
        return new NetIfaces.Candidate(name, up, multicast, loopback, Arrays.asList(ipv4));
    }

    private static Set<String> cellular(String... names) {
        return new HashSet<>(Arrays.asList(names));
    }

    /** A phone acting as hotspot: mobile data, the hotspot LAN and loopback. */
    private static List<NetIfaces.Candidate> phoneAsHotspot() {
        List<NetIfaces.Candidate> all = new ArrayList<>();
        all.add(nic("lo", true, false, true, "127.0.0.1"));
        all.add(nic("rmnet_data2", true, true, false, "100.64.14.9"));
        all.add(nic("swlan0", true, true, false, "10.20.89.13"));
        return all;
    }

    @Test
    public void theHotspotIsKeptAndCellularAndLoopbackAreDropped() {
        assertEquals(Collections.singletonList("10.20.89.13"),
                NetIfaces.select(phoneAsHotspot(), cellular("rmnet_data2")));
    }

    @Test
    public void cellularComesFromTheNetworkNotFromTheInterfaceName() {
        // Same phone, but the carrier interface is named like a Wi-Fi one and
        // the hotspot like a modem: only the cellular set decides.
        List<NetIfaces.Candidate> all = new ArrayList<>();
        all.add(nic("wlan0", true, true, false, "100.64.14.9"));
        all.add(nic("rmnet0", true, true, false, "10.20.89.13"));
        assertEquals(Collections.singletonList("10.20.89.13"),
                NetIfaces.select(all, cellular("wlan0")));
    }

    @Test
    public void downNonMulticastAndAddresslessInterfacesAreDropped() {
        List<NetIfaces.Candidate> all = new ArrayList<>();
        all.add(nic("ap0", false, true, false, "192.168.43.1"));
        all.add(nic("tun0", true, false, false, "10.8.0.2"));
        all.add(nic("wlan1", true, true, false));
        all.add(nic("p2p0", true, true, false, "192.168.49.1"));
        assertEquals(Collections.singletonList("192.168.49.1"),
                NetIfaces.select(all, cellular()));
    }

    @Test
    public void everyKeptInterfaceAndEveryAddressIsListedInOrder() {
        List<NetIfaces.Candidate> all = new ArrayList<>();
        all.add(nic("swlan0", true, true, false, "10.20.89.13", "192.168.1.5"));
        all.add(nic("p2p0", true, true, false, "192.168.49.1"));
        assertEquals(Arrays.asList("10.20.89.13", "192.168.1.5", "192.168.49.1"),
                NetIfaces.select(all, cellular()));
    }

    @Test
    public void noUsableInterfaceGivesAnEmptyList() {
        assertTrue(NetIfaces.select(phoneAsHotspot(), cellular("rmnet_data2", "swlan0"))
                .isEmpty());
        assertTrue(NetIfaces.select(Collections.<NetIfaces.Candidate>emptyList(), cellular())
                .isEmpty());
    }

    @Test
    public void theProbeGetsNameAddressAndPrefixSoReportsNameTheHotspot() {
        List<NetIfaces.Candidate> all = new ArrayList<>();
        all.add(nic("lo", true, false, true, "127.0.0.1"));
        all.add(new NetIfaces.Candidate("rmnet_data2", true, true, false,
                Arrays.asList("100.64.14.9"), Arrays.asList(30)));
        all.add(new NetIfaces.Candidate("swlan0", true, true, false,
                Arrays.asList("10.20.89.13", "192.168.1.5"), Arrays.asList(24, 16)));
        assertEquals("swlan0=10.20.89.13/24,swlan0=192.168.1.5/16",
                NetIfaces.selectLabelled(all, cellular("rmnet_data2")));
        assertEquals("", NetIfaces.selectLabelled(
                Collections.<NetIfaces.Candidate>emptyList(), cellular()));
    }

    @Test
    public void theListGoesToNativeCodeCommaSeparated() {
        assertEquals("10.20.89.13,192.168.49.1",
                NetIfaces.join(Arrays.asList("10.20.89.13", "192.168.49.1")));
        assertEquals("", NetIfaces.join(Collections.<String>emptyList()));
    }

    @Test
    public void aClatLeftAfterTheHotspotGoesOffIsNotUsable() {
        List<NetIfaces.Candidate> all = new ArrayList<>();
        all.add(nic("lo", true, false, true, "127.0.0.1"));
        all.add(nic("rmnet_data2", true, true, false, "100.64.14.9"));
        // The CLAT: green flags, 192.0.0.4, stacked under the cellular link.
        all.add(new NetIfaces.Candidate("v4-rmnet_data1", true, true, false,
                Arrays.asList("192.0.0.4"), Arrays.asList(29), true));
        // Even without the stacked flag the address alone cannot be a LAN.
        all.add(new NetIfaces.Candidate("v4-rmnet_data2", true, true, false,
                Arrays.asList("192.0.0.4"), Arrays.asList(29), false));
        assertTrue(NetIfaces.select(all, cellular("rmnet_data2")).isEmpty());
    }

    @Test
    public void aHotspotOnWithTheClatKeepsOnlyTheHotspotAddress() {
        List<NetIfaces.Candidate> all = new ArrayList<>();
        all.add(nic("rmnet_data2", true, true, false, "100.64.14.9"));
        all.add(new NetIfaces.Candidate("v4-rmnet_data1", true, true, false,
                Arrays.asList("192.0.0.4"), Arrays.asList(29), true));
        all.add(nic("swlan0", true, true, false, "10.20.89.13"));
        assertEquals(Collections.singletonList("10.20.89.13"),
                NetIfaces.select(all, cellular("rmnet_data2")));
    }

    @Test
    public void linkLocalAndStackedNonCellularAddressesAreNotUsable() {
        List<NetIfaces.Candidate> all = new ArrayList<>();
        all.add(new NetIfaces.Candidate("dummy0", true, true, false,
                Arrays.asList("169.254.7.7"), Arrays.asList(16), false));
        // A stacked link is excluded even when no cellular name matches it.
        all.add(new NetIfaces.Candidate("clat0", true, true, false,
                Arrays.asList("10.5.5.5"), Arrays.asList(24), true));
        assertTrue(NetIfaces.select(all, cellular()).isEmpty());
    }

    @Test
    public void theCandidateDescriptionNamesFlagsAndClassesButNeverAnAddress() {
        List<NetIfaces.Candidate> all = new ArrayList<>();
        all.add(new NetIfaces.Candidate("swlan0", true, true, false,
                Arrays.asList("10.20.89.13"), Arrays.asList(24), false));
        all.add(new NetIfaces.Candidate("v4-rmnet_data1", true, true, false,
                Arrays.asList("192.0.0.4"), Arrays.asList(29), true));
        all.add(new NetIfaces.Candidate("dummy0", true, true, false,
                Arrays.asList("169.254.7.7"), Arrays.asList(16), false));
        all.add(new NetIfaces.Candidate("rmnet_data2", true, true, false,
                Arrays.asList("100.64.14.9"), Arrays.asList(30), false));
        String text = NetIfaces.describe(all, cellular("rmnet_data2"));
        assertTrue(text.contains("name=swlan0,up=1,mcast=1,loop=0,cell=0,"
                + "stacked=0,addr=lan"));
        assertTrue(text.contains("name=v4-rmnet_data1,up=1,mcast=1,loop=0,"
                + "cell=0,stacked=1,addr=clat"));
        assertTrue(text.contains("name=dummy0,up=1,mcast=1,loop=0,"
                + "cell=0,stacked=0,addr=linklocal"));
        assertTrue(text.contains("name=rmnet_data2,up=1,mcast=1,loop=0,"
                + "cell=1,stacked=0,addr=other"));
        assertTrue(!text.contains("10.20.89.13"));
        assertTrue(!text.contains("192.0.0.4"));
        assertTrue(!text.contains("169.254.7.7"));
        assertTrue(!text.contains("100.64.14.9"));
    }

    @Test
    public void addressClassesAreTheFourDiagnosticWords() {
        assertEquals("clat", NetIfaces.addressClass("192.0.0.4"));
        assertEquals("linklocal", NetIfaces.addressClass("169.254.7.7"));
        assertEquals("lan", NetIfaces.addressClass("10.20.89.13"));
        assertEquals("lan", NetIfaces.addressClass("172.16.4.1"));
        assertEquals("lan", NetIfaces.addressClass("192.168.49.1"));
        assertEquals("other", NetIfaces.addressClass("172.32.4.1"));
        assertEquals("other", NetIfaces.addressClass("100.64.14.9"));
        assertEquals("other", NetIfaces.addressClass(""));
    }
}
