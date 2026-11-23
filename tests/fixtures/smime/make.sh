#!/bin/sh
# Makes the S/MIME fixtures smime.rs's tests read, with the openssl CLI
# (OpenSSL 3 or later). Run from this folder: sh make.sh. The keys are for
# tests only. Certificates last a century, so the tests don't expire.
set -eu
cd "$(dirname "$0")"
rm -f -- *.pem *.key *.eml *.der *.srl *.cnf
days=36500

cat > ext.cnf <<'EOF'
[ca]
basicConstraints = critical, CA:TRUE
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
[ana]
basicConstraints = CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = emailProtection
subjectAltName = email:ana@example.org
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
[bo]
basicConstraints = CA:FALSE
keyUsage = critical, digitalSignature, keyAgreement
extendedKeyUsage = emailProtection
subjectAltName = email:bo@example.se
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
EOF

# A test authority, Ana's RSA certificate and Bo's EC (P-256) one.
openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.pem -days $days \
    -subj "/CN=Picacho Test CA" -extensions ca -config ext.cnf 2>/dev/null
openssl req -new -newkey rsa:2048 -nodes -keyout ana.key -out ana.csr -subj "/CN=Ana" 2>/dev/null
openssl x509 -req -in ana.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out ana.pem \
    -days $days -extfile ext.cnf -extensions ana 2>/dev/null
openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout bo.key \
    -out bo.csr -subj "/CN=Bo" 2>/dev/null
openssl x509 -req -in bo.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out bo.pem \
    -days $days -extfile ext.cnf -extensions bo 2>/dev/null
rm -f ana.csr bo.csr ca.srl

printf 'Content-Type: text/plain; charset=utf-8\r\n\r\nMeet at noon.\r\n' > content.txt
headers='From: Ana <ana@example.org>\r\nTo: bo@example.se\r\nSubject: Plans\r\n'

# Signed by both, detached (multipart/signed), both certificates included.
{ printf "$headers"; openssl cms -sign -in content.txt -signer ana.pem -inkey ana.key \
    -signer bo.pem -inkey bo.key -md sha256 -binary; } > two-signers.eml
# Signed by both, with only Ana's certificate in the message.
{ printf "$headers"; openssl cms -sign -in content.txt -signer ana.pem -inkey ana.key \
    -signer bo.pem -inkey bo.key -md sha256 -binary -nocerts -certfile ana.pem; } \
    > one-certificate-missing.eml
# Signed by Bo, without his certificate.
{ printf "$headers"; openssl cms -sign -in content.txt -signer bo.pem -inkey bo.key \
    -md sha256 -binary -nocerts; } > certificate-missing.eml

# Signed opaquely (application/pkcs7-mime), streamed: BER, with
# indefinite lengths, as Outlook writes it.
{ printf "$headers"; openssl cms -sign -in content.txt -signer ana.pem -inkey ana.key \
    -md sha256 -binary -nodetach -stream; } > opaque-signed-ber.eml

# RFC 8551 §3.6: a certs-only message, Ana's and the authority's certificates.
openssl crl2pkcs7 -nocrl -certfile ana.pem -certfile ca.pem -outform DER -out certs-only.der
{ printf "$headers"; printf 'MIME-Version: 1.0\r\n'
  printf 'Content-Type: application/pkcs7-mime; smime-type=certs-only; name=smime.p7c\r\n'
  printf 'Content-Transfer-Encoding: base64\r\n\r\n'
  openssl base64 -in certs-only.der; } > certs-only.eml
rm -f certs-only.der

# RFC 3274: compressed, alone, inside a signature, and compressed twice.
{ printf "$headers"; openssl cms -compress -in content.txt; } > compressed.eml
openssl cms -compress -in content.txt | sed "s/\r*$/\r/" > compressed-part.txt
{ printf "$headers"; openssl cms -sign -in compressed-part.txt -signer ana.pem \
    -inkey ana.key -md sha256 -binary; } > signed-compressed.eml
{ printf "$headers"; openssl cms -compress -in compressed-part.txt; } > compressed-twice.eml
rm -f compressed-part.txt

# Old PKCS#7 mail: openssl smime writes application/x-pkcs7-signature.
{ printf "$headers"; openssl smime -sign -in content.txt -signer ana.pem -inkey ana.key \
    -md sha256 -binary; } > pkcs7-signed.eml
rm -f ext.cnf

# Encrypted to Ana with RSA-OAEP (RFC 8017 §7.1, RFC 3560), to Bo with
# ECDH (RFC 5753), and with AES-GCM as AuthEnvelopedData (RFC 5083/5084).
# The old PKCS#7 form, from openssl smime, with RSA PKCS#1 v1.5.
{ printf "$headers"; openssl cms -encrypt -in content.txt -aes-256-cbc -recip ana.pem \
    -keyopt rsa_padding_mode:oaep -binary; } > encrypted-rsa-oaep.eml
{ printf "$headers"; openssl cms -encrypt -in content.txt -aes-256-cbc -recip bo.pem \
    -binary; } > encrypted-ecdh.eml
{ printf "$headers"; openssl cms -encrypt -in content.txt -aes-256-gcm -recip ana.pem \
    -recip bo.pem -binary; } > encrypted-aes-gcm.eml
{ printf "$headers"; openssl smime -encrypt -in content.txt -aes-256-cbc -binary ana.pem; } \
    > pkcs7-encrypted.eml

# Mail travels with CRLF line ends (RFC 5322 §2.1); openssl writes LF
# outside the signed content, which already has CRLF.
for f in *.eml; do
    sed 's/\r*$/\r/' "$f" > "$f.tmp" && mv "$f.tmp" "$f"
done
