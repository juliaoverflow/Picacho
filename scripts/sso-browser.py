#!/usr/bin/env python3
"""Plays the person in the browser for Picacho's single sign-on test
(src/manage/sso_tests.rs): opens the authorization URL, follows Keycloak's
OpenID Connect → SAML 2.0 broker redirect, signs in at the SAML identity
provider's form, posts the SAML request and response on (as a browser's
auto-submitting forms would), and follows the redirects to the loopback
callback. Prints how many SAML messages it passed on.

Usage: sso-browser.py URL USER PASSWORD"""
import html
import re
import sys
import urllib.parse
import urllib.request

url, user, password = sys.argv[1:4]

# Keycloak's session cookies are Secure, which a test server on plain
# http://127.0.0.1 still needs sent back (browsers treat loopback as
# secure). Kept by (path, name) and sent where their path matches, as a
# browser does: both realms set cookies of the same names.
cookies = {}


def remember(request_url, headers):
    default = urllib.parse.urlparse(request_url).path.rsplit("/", 1)[0] or "/"
    for header in headers.get_all("Set-Cookie") or []:
        parts = [p.strip() for p in header.split(";")]
        name, _, value = parts[0].partition("=")
        attrs = {k.strip().lower(): v for k, _, v in (p.partition("=") for p in parts[1:])}
        path = attrs.get("path") or default
        if attrs.get("max-age") == "0" or value in ("", '""'):
            cookies.pop((path, name), None)
        else:
            cookies[(path, name)] = value


class Cookies(urllib.request.BaseHandler):
    def http_request(self, req):
        path = urllib.parse.urlparse(req.full_url).path
        sent = {}
        for (p, n), v in sorted(cookies.items(), key=lambda kv: -len(kv[0][0])):
            if path.startswith(p) and n not in sent:
                sent[n] = v
        if sent:
            req.add_unredirected_header("Cookie", "; ".join(f"{k}={v}" for k, v in sent.items()))
        return req

    def http_response(self, req, resp):
        remember(req.full_url, resp.headers)
        return resp

    https_request = http_request
    https_response = http_response


class Redirects(urllib.request.HTTPRedirectHandler):
    # Cookies set on a redirect are kept before it is followed.
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        remember(req.full_url, headers)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


opener = urllib.request.build_opener(Redirects, Cookies)


def fetch(u, data=None):
    body = urllib.parse.urlencode(data).encode() if data is not None else None
    with opener.open(urllib.request.Request(u, data=body)) as r:
        return r.geturl(), r.read().decode("utf-8", "replace")


def forms(page):
    for m in re.finditer(r"<form[^>]*>.*?</form>", page, re.S | re.I):
        f = m.group(0)
        action = re.search(r'action="([^"]*)"', f)
        inputs = {
            html.unescape(n): html.unescape(v)
            for n, v in re.findall(r'<input[^>]*name="([^"]+)"[^>]*value="([^"]*)"', f)
        }
        yield (html.unescape(action.group(1)) if action else None), inputs, f


saml_steps = 0
try:
    current, page = fetch(url)
    for _ in range(12):
        if "SAMLResponse" in page or "SAMLRequest" in page:
            saml_steps += 1
            action, inputs, _ = next(forms(page))
            current, page = fetch(urllib.parse.urljoin(current, action), inputs)
            continue
        login = [(a, i) for a, i, f in forms(page) if 'name="password"' in f]
        if login:
            action, inputs = login[0]
            inputs.update({"username": user, "password": password})
            current, page = fetch(urllib.parse.urljoin(current, action), inputs)
            continue
        break
except urllib.error.URLError as e:
    # The loopback callback answers once and closes, which ends the walk.
    print("ended", e)
print("saml steps", saml_steps)
print("final", current[:120] if "current" in dir() else "")
