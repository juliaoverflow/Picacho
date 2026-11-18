package tech.pinal.picacho;

import android.app.Activity;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.hardware.usb.UsbConstants;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbEndpoint;
import android.hardware.usb.UsbInterface;
import android.hardware.usb.UsbManager;
import android.nfc.NfcAdapter;
import android.nfc.Tag;
import android.nfc.tech.IsoDep;
import android.util.Log;
import java.io.IOException;
import java.util.HashSet;
import java.util.Set;

/**
 * Security keys for Rust (src/android/keys.rs), which speaks CTAP2 to them:
 * this finds one held to the phone (NFC reader mode, while Picacho is in
 * front) or plugged in (USB host, with the person's permission), and moves
 * its bytes. One key at a time, between start and stop.
 */
final class SecurityKeys {
    private static final String TAG = "picacho";
    private static final String ACTION_PERMISSION = "tech.pinal.picacho.USB_PERMISSION";
    /** What poll answers. */
    private static final int NONE = 0, USB = 1, NFC = 2, DENIED = 3;
    /** FIDO's HID usage page (0xF1D0) in a report descriptor: 06 D0 F1. */
    private static final byte[] FIDO_PAGE = {0x06, (byte) 0xD0, (byte) 0xF1};

    private static final Object lock = new Object();
    private static Context context;
    private static boolean searching;
    private static IsoDep nfc;
    private static UsbDeviceConnection usb;
    private static UsbInterface usbInterface;
    private static UsbEndpoint usbIn, usbOut;
    private static String usbName;
    /** Devices already asked about, so the person is asked once each. */
    private static final Set<String> asked = new HashSet<>();

    private SecurityKeys() {}

    /**
     * Called from Rust: starts looking. Answers whether NFC can be used: 0
     * the phone has none, 1 it's off, 2 it's on.
     */
    public static int start(Context appContext) {
        synchronized (lock) {
            closeKey();
            context = appContext;
            searching = true;
            asked.clear();
        }
        PicachoActivity activity = PicachoActivity.current();
        if (activity != null) {
            activity.runOnUiThread(() -> resumed(activity));
        }
        NfcAdapter adapter = NfcAdapter.getDefaultAdapter(appContext);
        if (adapter == null) {
            return 0;
        }
        return adapter.isEnabled() ? 2 : 1;
    }

    /** Called from Rust: whether a key is ready (see the constants). */
    public static int poll() {
        synchronized (lock) {
            if (!searching) {
                return NONE;
            }
            if (nfc != null) {
                return NFC;
            }
            if (usb != null) {
                return USB;
            }
            return findUsb();
        }
    }

    /** Called from Rust: stops looking, and lets go of the key. */
    public static void stop() {
        synchronized (lock) {
            searching = false;
            closeKey();
        }
        PicachoActivity activity = PicachoActivity.current();
        if (activity != null) {
            activity.runOnUiThread(() -> {
                NfcAdapter adapter = NfcAdapter.getDefaultAdapter(activity);
                if (adapter != null && !activity.isDestroyed()) {
                    try {
                        adapter.disableReaderMode(activity);
                    } catch (RuntimeException ignored) {
                    }
                }
            });
        }
    }

    /** PicachoActivity resumed (UI thread): reader mode, while looking. */
    static void resumed(Activity activity) {
        synchronized (lock) {
            if (!searching) {
                return;
            }
        }
        NfcAdapter adapter = NfcAdapter.getDefaultAdapter(activity);
        if (adapter == null || !adapter.isEnabled() || activity.isDestroyed()) {
            return;
        }
        int flags = NfcAdapter.FLAG_READER_NFC_A
                | NfcAdapter.FLAG_READER_NFC_B
                | NfcAdapter.FLAG_READER_SKIP_NDEF_CHECK
                | NfcAdapter.FLAG_READER_NO_PLATFORM_SOUNDS;
        try {
            adapter.enableReaderMode(activity, SecurityKeys::tagFound, flags, null);
        } catch (RuntimeException e) {
            Log.w(TAG, "NFC reader mode", e);
        }
    }

    /** A tag came near (reader mode's thread). */
    private static void tagFound(Tag tag) {
        IsoDep dep = IsoDep.get(tag);
        if (dep == null) {
            return;
        }
        try {
            dep.connect();
            // A key may take a while, its user verification included.
            dep.setTimeout(10_000);
        } catch (IOException e) {
            return;
        }
        synchronized (lock) {
            if (!searching || nfc != null || usb != null) {
                closeQuietly(dep);
                return;
            }
            nfc = dep;
        }
    }

    /** Called from Rust: an APDU to the key held to the phone; null if it's gone. */
    public static byte[] nfcTransceive(byte[] apdu) {
        IsoDep dep;
        synchronized (lock) {
            dep = nfc;
        }
        if (dep == null) {
            return null;
        }
        try {
            return dep.transceive(apdu);
        } catch (IOException | RuntimeException e) {
            return null;
        }
    }

    /** Called from Rust: one 64-byte HID report to the plugged-in key. */
    public static boolean usbWrite(byte[] report) {
        UsbDeviceConnection connection;
        UsbEndpoint out;
        synchronized (lock) {
            connection = usb;
            out = usbOut;
        }
        return connection != null
                && connection.bulkTransfer(out, report, report.length, 1000) == report.length;
    }

    /**
     * Called from Rust: the key's next report; null if none came within
     * {@code timeoutMs}, an empty array if the key is gone.
     */
    public static byte[] usbRead(int timeoutMs) {
        UsbDeviceConnection connection;
        UsbEndpoint in;
        synchronized (lock) {
            connection = usb;
            in = usbIn;
        }
        if (connection == null) {
            return new byte[0];
        }
        byte[] report = new byte[Math.max(64, in.getMaxPacketSize())];
        int read = connection.bulkTransfer(in, report, report.length, timeoutMs);
        if (read < 0) {
            // A timeout, or the key was pulled out.
            return stillPlugged() ? null : new byte[0];
        }
        return report;
    }

    private static boolean stillPlugged() {
        UsbManager manager;
        String name;
        synchronized (lock) {
            if (context == null || usb == null) {
                return false;
            }
            manager = context.getSystemService(UsbManager.class);
            name = usbName;
        }
        return manager != null && manager.getDeviceList().containsKey(name);
    }

    /** Looks for a plugged-in FIDO key, asking permission once. Holds lock. */
    private static int findUsb() {
        UsbManager manager = context == null ? null : context.getSystemService(UsbManager.class);
        if (manager == null) {
            return NONE;
        }
        boolean denied = false;
        for (UsbDevice device : manager.getDeviceList().values()) {
            UsbInterface hid = fidoInterface(device);
            if (hid == null) {
                continue;
            }
            if (!manager.hasPermission(device)) {
                if (asked.add(device.getDeviceName())) {
                    Intent intent = new Intent(ACTION_PERMISSION).setPackage(context.getPackageName());
                    PendingIntent permission = PendingIntent.getBroadcast(
                            context, 0, intent, PendingIntent.FLAG_IMMUTABLE);
                    manager.requestPermission(device, permission);
                } else {
                    denied = true;
                }
                continue;
            }
            if (open(manager, device, hid)) {
                return USB;
            }
        }
        // Asked, and the dialog is gone without permission.
        return denied && !waitingForAnswer() ? DENIED : NONE;
    }

    /** Whether the permission dialog may still be showing: Picacho isn't resumed. */
    private static boolean waitingForAnswer() {
        PicachoActivity activity = PicachoActivity.current();
        return activity == null || !activity.hasWindowFocus();
    }

    /**
     * A HID interface with interrupt endpoints both ways, as FIDO keys have
     * (keyboards and mice only report in), or null.
     */
    private static UsbInterface fidoInterface(UsbDevice device) {
        for (int i = 0; i < device.getInterfaceCount(); i++) {
            UsbInterface candidate = device.getInterface(i);
            if (candidate.getInterfaceClass() != UsbConstants.USB_CLASS_HID) {
                continue;
            }
            UsbEndpoint in = null, out = null;
            for (int e = 0; e < candidate.getEndpointCount(); e++) {
                UsbEndpoint endpoint = candidate.getEndpoint(e);
                if (endpoint.getType() != UsbConstants.USB_ENDPOINT_XFER_INT) {
                    continue;
                }
                if (endpoint.getDirection() == UsbConstants.USB_DIR_IN) {
                    in = endpoint;
                } else {
                    out = endpoint;
                }
            }
            if (in != null && out != null) {
                return candidate;
            }
        }
        return null;
    }

    /** Opens the key, if its report descriptor says FIDO. Holds lock. */
    private static boolean open(UsbManager manager, UsbDevice device, UsbInterface hid) {
        UsbDeviceConnection connection = manager.openDevice(device);
        if (connection == null) {
            return false;
        }
        if (!connection.claimInterface(hid, true)) {
            connection.close();
            return false;
        }
        // GET_DESCRIPTOR (HID report descriptor) for the interface.
        byte[] descriptor = new byte[512];
        int length = connection.controlTransfer(
                0x81, 0x06, 0x2200, hid.getId(), descriptor, descriptor.length, 1000);
        if (length > 0 && !contains(descriptor, length, FIDO_PAGE)) {
            connection.releaseInterface(hid);
            connection.close();
            return false;
        }
        UsbEndpoint in = null, out = null;
        for (int e = 0; e < hid.getEndpointCount(); e++) {
            UsbEndpoint endpoint = hid.getEndpoint(e);
            if (endpoint.getType() != UsbConstants.USB_ENDPOINT_XFER_INT) {
                continue;
            }
            if (endpoint.getDirection() == UsbConstants.USB_DIR_IN) {
                in = endpoint;
            } else {
                out = endpoint;
            }
        }
        usb = connection;
        usbInterface = hid;
        usbIn = in;
        usbOut = out;
        usbName = device.getDeviceName();
        return true;
    }

    private static boolean contains(byte[] bytes, int length, byte[] part) {
        outer:
        for (int i = 0; i + part.length <= length; i++) {
            for (int j = 0; j < part.length; j++) {
                if (bytes[i + j] != part[j]) {
                    continue outer;
                }
            }
            return true;
        }
        return false;
    }

    /** Lets go of the key found, if any. Holds lock. */
    private static void closeKey() {
        if (nfc != null) {
            closeQuietly(nfc);
            nfc = null;
        }
        if (usb != null) {
            if (usbInterface != null) {
                usb.releaseInterface(usbInterface);
            }
            usb.close();
            usb = null;
            usbInterface = null;
            usbIn = null;
            usbOut = null;
            usbName = null;
        }
    }

    private static void closeQuietly(IsoDep dep) {
        try {
            dep.close();
        } catch (IOException ignored) {
        }
    }
}
