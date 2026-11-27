# Security keys (src/manage/hardware.rs, src/manage/openpgp/card.rs,
# src/manage/smime/token.rs, src/sync/pcsc.rs) and where Settings →
# Encryption and the reader show them.

security-keys-no-reader = No smart card reader or security key is connected.

security-keys-no-pcsc-service = The smart card service isn't running (pcscd on Linux).

security-keys-no-card-in-reader = There's no card in the reader.

security-keys-pcsc-error = The smart card service answered { $code }.

security-keys-no-pcsc-library = The smart card service isn't installed ({ $error }). On Linux, install pcsc-lite and start pcscd; in a Flatpak, Picacho needs access to it.

security-keys-not-on-this-device = Security keys over USB aren't available on this device.

security-keys-no-pinpad = Readers with their own PIN pad aren't supported.

security-keys-insert-the-card = Connect the security key { $ident }.

security-keys-insert-the-token = Connect the security key or insert the smart card.

security-keys-enter-the-pin = Enter the security key's PIN.

security-keys-wrong-pin = { $tries ->
        [0] The PIN is wrong, and the key has blocked it. Unblock it with its PUK or admin PIN.
        [one] The PIN is wrong. One try is left before the key blocks it.
       *[other] The PIN is wrong. { $tries } tries are left before the key blocks it.
    }

security-keys-the-card-refused-the-pin = The security key refused the PIN. Unlock it again.

security-keys-the-token-refused-the-pin = The PIN is wrong. Each wrong try counts toward blocking it.

security-keys-the-pin-is-blocked = The PIN is blocked. Unblock it with the key's own tools.

security-keys-unlock-to-use = Unlock the security key { $name } to use it.

security-keys-the-card-doesnt-sign = This security key has no signing key.

security-keys-the-card-holds-another-key = The security key holds another key than the one added. Remove it and add it again.

security-keys-the-certificate-isnt-the-cards = That certificate isn't for the keys on this security key.

security-keys-no-module-there = There's no PKCS#11 module at { $path }.

security-keys-module-wont-load = The PKCS#11 module couldn't be loaded: { $error }

security-keys-no-identity-on-the-token = The security key holds no S/MIME certificate for an email address with its private key.

security-keys-the-key-isnt-on-the-token = The private key is no longer on the security key.

security-keys-paste-the-certificate = Picacho couldn't find this key's certificate (its public key) here, at the key's URL or on keys.openpgp.org. Import it with Import Keys (exported from GnuPG or your key's app), then add the security key again.

security-keys-no-longer-added = That security key is no longer added.

security-keys-title = Security keys

security-keys-none-yet = No security keys yet. Your keys on a YubiKey, Nitrokey or smart card sign and decrypt there, and never leave it.

security-keys-pin = PIN

security-keys-pin-caption = The security key's PIN, for unlocking or adding it. It is kept in memory only, until unused for 30 minutes or Picacho locks; wrong PINs count toward blocking the key.

security-keys-pin-placeholder = PIN

security-keys-openpgp-cards = OpenPGP cards

security-keys-openpgp-cards-caption = YubiKey, Nitrokey, Gnuk and other OpenPGP cards, over the smart card service. Your key's certificate is found here, at the card's URL or on keys.openpgp.org.

security-keys-find-cards = Find Security Keys

security-keys-looking = Looking…

security-keys-no-new-cards = No OpenPGP card that isn't added already is connected.

security-keys-card-facts = { $ident } · { $tries ->
        [one] 1 PIN try left
       *[other] { $tries } PIN tries left
    }

security-keys-add = Add

security-keys-added = { $name } was added

security-keys-pkcs11 = PKCS#11 tokens (S/MIME)

security-keys-pkcs11-caption = A PKCS#11 module, such as OpenSC's or Yubico's ykcs11, reaches the S/MIME certificates on a smart card or a YubiKey's PIV.

security-keys-pkcs11-found = A PKCS#11 module, such as OpenSC's or Yubico's ykcs11, reaches the S/MIME certificates on a smart card or a YubiKey's PIV. Found here: { $modules }

security-keys-module-placeholder = Path to the PKCS#11 module

security-keys-add-from-token = Add Certificates

security-keys-certificates-added = { $count ->
        [one] 1 certificate was added
       *[other] { $count } certificates were added
    }

security-keys-kind-openpgp = OpenPGP card

security-keys-kind-smime = S/MIME token

security-keys-locked = Locked

security-keys-unlocked = Unlocked

security-keys-unlock = Unlock

security-keys-unlock-named = Unlock { $name }

security-keys-lock = Lock

security-keys-remove = Remove

security-keys-message-needs-key = This message is encrypted to your security key. Enter its PIN to read it.

security-keys-on-key = On { $name }
