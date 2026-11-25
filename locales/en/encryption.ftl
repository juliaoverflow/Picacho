# src/display/encryption.rs and nearby views.

encryption-never = Never

encryption-1-year = 1 year

encryption-years = { $n } years

encryption-good = Good

encryption-untrusted-no-trusted-authority-vouches-for = Untrusted: no trusted authority vouches for the certificate

encryption-unverified-the-message-doesnt-include-the = Unverified: the message doesn't include the certificate

encryption-revoked-the-certificate-was-withdrawn = Revoked: the certificate was withdrawn

encryption-bad-the-signature-doesnt-match = Bad: the signature doesn't match

encryption-expired = expired { $date }

encryption-expires = expires { $date }

encryption-your-key = your key

encryption-public-key = public key

encryption-revoked = revoked

encryption-no-name = (no name)

encryption-revoked-2 = Revoked

encryption-your-key-2 = Your key

encryption-found-automatically-checked-by-you = Found automatically, checked by you

encryption-found-automatically-not-checked-compare-the = Found automatically, not checked: compare the fingerprint with its owner

encryption-public-key-2 = Public key

encryption-remove-expiry = Remove Expiry

encryption-renew = Renew…

encryption-show = Show

encryption-fingerprint = Fingerprint

encryption-check = Check…

encryption-remove = Remove

encryption-no-keys-yet = No keys yet.

encryption-openpgp-keys = OpenPGP keys

encryption-with-your-key-picacho-opens-mail = With your key, Picacho opens mail encrypted to you and signs what you send; with others' public keys, it checks their signatures and encrypts to them. Choose Sign or Encrypt in the composer's menu. Pinal Mail's own encryption needs nothing here.

encryption-import-keys = Import keys

encryption-choose-armored-key-files-asc-a = Choose armored key files (.asc). A secret key with a passphrase needs it once: it's then kept, without the passphrase, in the system keychain.

encryption-new-and-renewed-keys-expire = New and renewed keys expire:

encryption-import-keys-2 = Import Keys…

encryption-create-a-key = Create a Key

encryption-share-your-public-key = Share your public key

encryption-save-it-to-send-to-people = Save it to send to people who'll write to you encrypted, publish it on keys.openpgp.org (which lists an address only once its owner confirms it by email) or on an HKP keyserver (which lists every address on it at once, for good), or export Web Key Directory (WKD) files to host on your own domain.

encryption-save-public-keys = Save Public Keys…

encryption-publish-on-keys-openpgp-org = Publish on keys.openpgp.org

encryption-export-for-wkd = Export for WKD…

encryption-publish-on-keyserver = Publish on Keyserver…

encryption-ask-for-encrypted-replies = Ask for encrypted replies ({ $email })

encryption-tells-peoples-mail-apps-in-the = Tells people's mail apps (in the Autocrypt header of mail you send) that you'd like them to encrypt to you by default. Their app decides; apps that don't use Autocrypt ignore it.

encryption-find-keys = Find keys

encryption-look-up-the-keys-of-people = Look up the keys of people you've written to or heard from, on their domain (WKD) and keys.openpgp.org. Picacho also looks when you send encrypted mail to someone without a key, and keeps the keys people's mail apps send along (Autocrypt). Keys you imported yourself are never replaced.

encryption-looking = Looking…

encryption-find-key = Find Key

encryption-certificates-many-companies-and-authorities-issue = Certificates many companies and authorities issue. Import yours as a .p12 or .pfx file (with its password); others' certificates are kept from the signed mail they send you, or imported as .pem, .crt or .cer. In the composer, choose S/MIME in place of OpenPGP.

encryption-import-certificates = Import certificates

encryption-your-own-p12-pfx-with-the = Your own (.p12, .pfx) with the password above, or someone else's (.pem, .crt, .cer).

encryption-import-certificates-2 = Import Certificates…

encryption-signers = Signers

encryption-certificates-in-this-message = Certificates in this message

encryption-this-message-carries-certificates = { $count ->
        [one] This message carries { $count } certificate
       *[other] This message carries { $count } certificates
    }

encryption-imported-they-let-you-write-to = Imported, they let you write to their owners encrypted. Importing vouches for nothing: anyone can send a certificate, and a signature by one is trusted only when an authority your system trusts issued it.

encryption-import-certificates-3 = Import Certificates

encryption-revocation-lists-crl = Revocation lists (CRL)

encryption-lists-of-withdrawn-certificates-as-your = Lists of withdrawn certificates, as your organization or a certificate authority publishes them (.crl or .pem). Signatures are then checked against them; a copy of each is kept here, so update it by adding the new one.

encryption-revocation-lists = Revocation lists

encryption-revoked-3 = { $revoked } revoked

encryption-an-unnamed-issuer = An unnamed issuer

encryption-add-revocation-lists = Add Revocation Lists…

encryption-save-revocation-certificate = Save Revocation Certificate…

encryption-keep-it-somewhere-safe-away-from = Keep it somewhere safe, away from this device. Importing it, here or in any app, revokes this key for good: use it only if the key is lost or someone else may have it.

encryption-autocrypt-setup-message = Autocrypt setup message

encryption-your-encryption-key-from-another-mail = Your encryption key, from another mail app

encryption-type-the-setup-code-that-app = Type the setup code that app showed you (36 digits) to use the same key here.

encryption-import-key = Import Key

encryption-import = Import

encryption-isnt-an-armored-key-file = { $path } isn't an armored key file.

encryption-imported-keys = { $n ->
        [one] Imported { $n } key
       *[other] Imported { $n } keys
    }

encryption-your-key-from-the-other-app = Your key from the other app is ready to use

encryption-add-an-account-first-the-key = Add an account first: the key is for its address.

encryption-your-new-key-is-ready-save = Your new key is ready. Save your public key to share it.

encryption-renewed-share-your-public-key-again = Renewed. Share your public key again so others see the new date.

encryption-the-fingerprints-match-the-key-is = The fingerprints match: the key is checked

encryption-key-marked-as-checked = Key marked as checked

encryption-key-removed = Key removed

encryption-public-key-saved = Public key saved

encryption-revocation-certificate-saved-keep-it-somewhere = Revocation certificate saved. Keep it somewhere safe.

encryption-found-the-key-on-keys-openpgp = Found the key on keys.openpgp.org

encryption-enter-an-email-address-or-a = Enter an email address or a key's fingerprint to look it up.

encryption-no-key-was-found-for = No key was found for { $email }.

encryption-found-the-key-for-on = Found the key for { $email } on { $label }

encryption-imported-certificates = { $n ->
        [one] Imported { $n } certificate
       *[other] Imported { $n } certificates
    }

# After "Imported 3 certificates", what was left out
encryption-without-an-email-address-an-authoritys = { $skipped ->
        [one] { $skipped } certificate without an email address (an authority's) left out
       *[other] { $skipped } certificates without an email address (an authority's) left out
    }

# The two messages above, one after the other
encryption-imported-and-left-out = { $imported }; { $left_out }

encryption-add = Add

encryption-couldnt-be-added = { $path } couldn't be added.

encryption-added-revocation = { $n ->
        [one] Added { $n } revocation list
       *[other] Added { $n } revocation lists
    }

encryption-revocation-list-removed = Revocation list removed

encryption-certificate-removed = Certificate removed

encryption-fingerprint-of = Fingerprint of { $who }

encryption-your-fingerprint = Your fingerprint

encryption-check-2 = Check { $who }

encryption-read-it-out-to-people-who = Read it out to people who write to you encrypted, or let them scan the code, so they know the key they have is yours.

encryption-ask-them-for-their-fingerprint-in = Ask them for their fingerprint (in person, on a call, or anywhere other than this mail) and compare. If they paste it to you, paste it below.

encryption-fingerprint-2 = Fingerprint { $fingerprint }

encryption-qr-code-of-this-keys-fingerprint = QR code of this key's fingerprint

encryption-compare = Compare

encryption-they-match-compared-in-person = They Match (compared in person)

encryption-no-certificates-yet = No certificates yet.

encryption-expires-2 = { $who }, { $own ->
        [yes] your certificate
       *[no] certificate
    }, expires { $expires }

encryption-issued-by-expires = { $kind } · issued by { $issuer } · expires { $expires }

encryption-s-mime-certificates = S/MIME certificates

encryption-standard-v4 = Standard (v4)

encryption-modern-v6 = Modern (v6)

encryption-key-type = Key type:

encryption-modern-keys-arent-yet-readable-by = Modern keys aren't yet readable by many mail apps. Choose Standard unless your correspondents use OpenPGP v6.

encryption-out-of-date-since-add-a = out of date since { $d }: add a newer one

encryption-next-update = next update { $d }

encryption-your-certificate-2 = Your certificate

encryption-certificate = Certificate

encryption-an-unnamed-issuer-2 = an unnamed issuer

encryption-you-already-have-a-key-for = You already have a key for { $email }.

encryption-no-key-with-that-fingerprint-was = No key with that fingerprint was found.

encryption-the-key-found-couldnt-be-read = The key found couldn't be read.

encryption-you-already-have-a-key-for-2 = You already have a key for { $email } that you imported yourself.
