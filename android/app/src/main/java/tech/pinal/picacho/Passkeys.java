package tech.pinal.picacho;

import android.annotation.TargetApi;
import android.credentials.CredentialManager;
import android.credentials.CredentialOption;
import android.credentials.GetCredentialException;
import android.credentials.GetCredentialRequest;
import android.credentials.GetCredentialResponse;
import android.os.Build;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.os.OutcomeReceiver;
import android.util.Log;
import android.util.SparseArray;

/**
 * Passkeys for Rust (src/android/keys.rs), from Android's Credential Manager
 * (Android 14 and later), without the androidx library: its public-key
 * option is a Bundle with these keys. Picacho asks as a browser does, for
 * another site's origin with that site's client data hash
 * (CREDENTIAL_MANAGER_SET_ORIGIN); the person's password manager decides
 * whether to answer apps that do.
 *
 * <p>Answers Rust's request with "ok:" and the WebAuthn JSON, or one of
 * "cancelled", "none", "unsupported", "refused", "closed" (Picacho isn't
 * open) or "error:" and why. Rust words the messages.
 */
final class Passkeys {
    private static final String TAG = "picacho";
    private static final String TYPE = "androidx.credentials.TYPE_PUBLIC_KEY_CREDENTIAL";
    private static final String KEY_SUBTYPE = "androidx.credentials.BUNDLE_KEY_SUBTYPE";
    private static final String SUBTYPE =
            "androidx.credentials.BUNDLE_VALUE_SUBTYPE_GET_PUBLIC_KEY_CREDENTIAL_OPTION";
    private static final String KEY_REQUEST = "androidx.credentials.BUNDLE_KEY_REQUEST_JSON";
    private static final String KEY_HASH = "androidx.credentials.BUNDLE_KEY_CLIENT_DATA_HASH";
    private static final String KEY_RESPONSE =
            "androidx.credentials.BUNDLE_KEY_AUTHENTICATION_RESPONSE_JSON";

    /** Requests in progress, to cancel. */
    private static final SparseArray<CancellationSignal> pending = new SparseArray<>();

    private Passkeys() {}

    /** Called from Rust; answers request {@code id} later, on any thread. */
    public static void get(int id, String requestJson, String origin, byte[] clientDataHash) {
        if (Build.VERSION.SDK_INT < 34) {
            Native.answer(id, "unsupported");
            return;
        }
        PicachoActivity activity = PicachoActivity.current();
        if (activity == null) {
            Native.answer(id, "closed");
            return;
        }
        activity.runOnUiThread(() -> request(activity, id, requestJson, origin, clientDataHash));
    }

    /** Called from Rust: the sign-in was cancelled. */
    public static void cancel(int id) {
        CancellationSignal signal;
        synchronized (pending) {
            signal = pending.get(id);
            pending.remove(id);
        }
        if (signal != null) {
            signal.cancel();
        }
    }

    @TargetApi(34)
    private static void request(
            PicachoActivity activity, int id, String json, String origin, byte[] hash) {
        Bundle data = new Bundle();
        data.putString(KEY_SUBTYPE, SUBTYPE);
        data.putString(KEY_REQUEST, json);
        data.putByteArray(KEY_HASH, hash);
        CancellationSignal signal = new CancellationSignal();
        synchronized (pending) {
            pending.put(id, signal);
        }
        try {
            CredentialOption option = new CredentialOption.Builder(TYPE, data, data).build();
            GetCredentialRequest request = new GetCredentialRequest.Builder(new Bundle())
                    .addCredentialOption(option)
                    .setOrigin(origin)
                    .build();
            CredentialManager manager = activity.getSystemService(CredentialManager.class);
            if (manager == null) {
                finish(id, "unsupported");
                return;
            }
            manager.getCredential(activity, request, signal, activity.getMainExecutor(),
                    new OutcomeReceiver<GetCredentialResponse, GetCredentialException>() {
                        @Override
                        public void onResult(GetCredentialResponse response) {
                            String answer = response.getCredential().getData().getString(KEY_RESPONSE);
                            finish(id, answer == null ? "error:no answer" : "ok:" + answer);
                        }

                        @Override
                        public void onError(GetCredentialException e) {
                            finish(id, describe(e));
                        }
                    });
        } catch (SecurityException e) {
            // Not allowed to ask for another site's passkeys.
            Log.w(TAG, "passkey request refused", e);
            finish(id, "refused");
        } catch (RuntimeException e) {
            Log.w(TAG, "passkey request failed", e);
            finish(id, "error:" + e.getMessage());
        }
    }

    @TargetApi(34)
    private static String describe(GetCredentialException e) {
        String type = e.getType();
        if (GetCredentialException.TYPE_USER_CANCELED.equals(type)
                || GetCredentialException.TYPE_INTERRUPTED.equals(type)) {
            return "cancelled";
        }
        if (GetCredentialException.TYPE_NO_CREDENTIAL.equals(type)) {
            return "none";
        }
        String message = e.getMessage();
        // Password managers say this way that they don't know the caller.
        if (message != null && message.toLowerCase().contains("privileged")) {
            return "refused";
        }
        return "error:" + (message != null ? message : type);
    }

    private static void finish(int id, String answer) {
        synchronized (pending) {
            pending.remove(id);
        }
        Native.answer(id, answer);
    }
}
