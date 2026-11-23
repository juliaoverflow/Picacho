#!/bin/sh
# Sets up Keycloak for the single sign-on test (src/manage/sso_tests.rs):
#   - realm `corp`: the organization's SAML 2.0 identity provider, with the
#     user ana@example.edu (password ana-password);
#   - realm `mail`: what Picacho signs in to (OpenID Connect), with the public
#     client `picacho`, which sends everyone to `corp` over SAML.
#
# Usage: KEYCLOAK_HOME=/path/to/keycloak-26.x sh scripts/keycloak-sso.sh
# It starts Keycloak in development mode on http://127.0.0.1:18080 unless one
# answers there already, sets it up, and leaves it running (stop it with
# `pkill -f keycloak`). Then:
#   cargo test -- --ignored sso
set -eu
: "${KEYCLOAK_HOME:?set KEYCLOAK_HOME to an unpacked Keycloak release}"
B=${PICACHO_KEYCLOAK:-http://127.0.0.1:18080}
PORT=${B##*:}
K="$KEYCLOAK_HOME/bin/kcadm.sh"
# Its admin session, in a directory of this user's own (not a fixed name in
# a shared /tmp another user could make first).
state="${XDG_RUNTIME_DIR:-$HOME/.cache}/picacho-keycloak"
mkdir -p -m 700 "$state"
export KCADM_CONFIG="$state/kcadm.config"

up() { curl -s -o /dev/null -w '%{http_code}' "$B/realms/master" | grep -q 200; }
if ! up; then
    KC_BOOTSTRAP_ADMIN_USERNAME=admin KC_BOOTSTRAP_ADMIN_PASSWORD=admin \
        nohup "$KEYCLOAK_HOME/bin/kc.sh" start-dev --http-port="$PORT" \
        >"$state/keycloak.log" 2>&1 &
    i=0
    until up; do
        i=$((i + 1))
        [ $i -lt 90 ] || { echo "Keycloak didn't start" >&2; exit 1; }
        sleep 2
    done
fi
"$K" config credentials --server "$B" --realm master --user admin --password admin

if "$K" get realms/mail >/dev/null 2>&1; then
    echo "Already set up."
    exit 0
fi

# The organization's SAML identity provider and its person.
"$K" create realms -s realm=corp -s enabled=true
"$K" create users -r corp -s username=ana -s email=ana@example.edu \
    -s emailVerified=true -s firstName=Ana -s lastName=Diaz -s enabled=true
"$K" set-password -r corp --username ana --new-password ana-password

# The realm mail apps sign in to, and Picacho's public client (PKCE).
"$K" create realms -s realm=mail -s enabled=true
"$K" create clients -r mail -s clientId=picacho -s publicClient=true \
    -s standardFlowEnabled=true -s 'redirectUris=["http://127.0.0.1/*"]' \
    -s 'attributes={"pkce.code.challenge.method":"S256"}'

# `corp` knows `mail` as a SAML service provider…
"$K" create clients -r corp -f - <<EOF
{"clientId":"$B/realms/mail","protocol":"saml","enabled":true,
 "redirectUris":["$B/realms/mail/broker/corp-saml/endpoint"],
 "attributes":{"saml.assertion.signature":"true","saml.server.signature":"true",
  "saml.client.signature":"false","saml_force_name_id_format":"true",
  "saml_name_id_format":"email",
  "saml_assertion_consumer_url_post":"$B/realms/mail/broker/corp-saml/endpoint"},
 "protocolMappers":[
  {"name":"email","protocol":"saml","protocolMapper":"saml-user-property-mapper",
   "config":{"user.attribute":"email","attribute.name":"email","attribute.nameformat":"Basic"}},
  {"name":"firstName","protocol":"saml","protocolMapper":"saml-user-property-mapper",
   "config":{"user.attribute":"firstName","attribute.name":"firstName","attribute.nameformat":"Basic"}},
  {"name":"lastName","protocol":"saml","protocolMapper":"saml-user-property-mapper",
   "config":{"user.attribute":"lastName","attribute.name":"lastName","attribute.nameformat":"Basic"}}]}
EOF

# …and `mail` trusts `corp`'s signed SAML assertions.
CERT=$(curl -s "$B/realms/corp/protocol/saml/descriptor" |
    sed -n 's/.*<ds:X509Certificate>\([^<]*\)<.*/\1/p' | head -n 1)
"$K" create identity-provider/instances -r mail -f - <<EOF
{"alias":"corp-saml","providerId":"saml","enabled":true,"trustEmail":true,
 "firstBrokerLoginFlowAlias":"first broker login",
 "config":{"singleSignOnServiceUrl":"$B/realms/corp/protocol/saml",
  "nameIDPolicyFormat":"urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress",
  "principalType":"ATTRIBUTE","principalAttribute":"email",
  "postBindingResponse":"true","postBindingAuthnRequest":"true",
  "validateSignature":"true","signingCertificate":"$CERT",
  "wantAssertionsSigned":"true","entityId":"$B/realms/mail","syncMode":"FORCE"}}
EOF
for a in email firstName lastName; do
    "$K" create identity-provider/instances/corp-saml/mappers -r mail \
        -s name=$a -s identityProviderAlias=corp-saml \
        -s identityProviderMapper=saml-user-attribute-idp-mapper \
        -s "config={\"attribute.name\":\"$a\",\"user.attribute\":\"$a\",\"syncMode\":\"FORCE\"}"
done

# Everyone signing in to `mail` goes straight to `corp`.
EXECUTION=$("$K" get authentication/flows/browser/executions -r mail |
    python3 -c "import sys, json; print([e['id'] for e in json.load(sys.stdin) if e.get('providerId') == 'identity-provider-redirector'][0])")
"$K" create "authentication/executions/$EXECUTION/config" -r mail \
    -s alias=to-corp -s 'config={"defaultProvider":"corp-saml"}'
echo "Set up: issuer $B/realms/mail, client picacho, user ana / ana-password."
