"""Synthetic mail for training the sign-in code model.

Each generated message is a subject and body built from templates; every
candidate in it is labelled: 1 for the sign-in or verification code, 0 for
any other number (order numbers, amounts, dates, phone numbers, postcodes,
account endings, tracking and booking references...). Templates are split
into two halves by `split`, so the held-out half measures how the model does
on wording it never saw.
"""

import random
import string

SERVICES = [
    "Google", "Microsoft", "Apple", "Amazon", "GitHub", "Facebook", "Instagram",
    "Twitter", "X", "LinkedIn", "Slack", "Discord", "Dropbox", "PayPal", "Stripe",
    "Coinbase", "Binance", "Steam", "Epic Games", "Netflix", "Spotify", "Uber",
    "Airbnb", "Shopify", "Notion", "Figma", "Zoom", "Atlassian", "Okta", "Auth0",
    "Proton", "Fastmail", "Revolut", "Wise", "N26", "Chase", "Bank of America",
    "Wells Fargo", "HSBC", "Barclays", "Monzo", "Robinhood", "Kraken", "Twitch",
    "Reddit", "TikTok", "Snapchat", "WhatsApp", "Telegram", "Signal", "Adobe",
    "Canva", "Booking.com", "Expedia", "eBay", "Etsy", "Walmart", "Target",
    "Acme Corp", "Northwind", "Contoso", "Picacho Cloud", "MyBank", "Hostinger",
    "DigitalOcean", "Cloudflare", "Vercel", "Netlify", "Heroku", "OpenAI", "Duolingo",
]
NAMES = ["Alex", "Sam", "Jordan", "Taylor", "Morgan", "Chris", "Pat", "Robin",
         "Jamie", "Casey", "Ana", "Luis", "Mei", "Ravi", "Olga", "Kofi", "Yuki"]
CITIES = ["Springfield, IL 62701", "Mountain View, CA 94043", "Seattle, WA 98109",
          "Austin, TX 78701", "London EC1A 1BB", "Dublin 2, D02 X285",
          "Berlin 10115", "Paris 75008", "Toronto, ON M5V 2T6", "Madrid 28013"]
STREETS = ["1600 Amphitheatre Parkway", "1 Infinite Loop", "410 Terry Ave N",
           "221B Baker Street", "500 Oracle Pkwy", "1355 Market Street, Suite 900",
           "4059 Mt Lee Dr", "3500 Deer Creek Road"]
MINUTES = ["5", "10", "15", "30", "60"]


def code(rng, kind=None):
    kind = kind or rng.choice(["d6", "d6", "d6", "d6", "d4", "d5", "d7", "d8",
                               "g6", "split", "alnum6", "alnum8"])
    d = lambda n: "".join(rng.choice(string.digits) for _ in range(n))
    if kind == "d4":
        return d(4)
    if kind == "d5":
        return d(5)
    if kind == "d6":
        return d(6)
    if kind == "d7":
        return d(7)
    if kind == "d8":
        return d(8)
    if kind == "g6":
        return "G-" + d(6)
    if kind == "split":
        return d(3) + rng.choice([" ", "-"]) + d(3)
    chars = string.ascii_uppercase + string.digits
    n = 6 if kind == "alnum6" else 8
    while True:
        c = "".join(rng.choice(chars) for _ in range(n))
        if sum(ch.isdigit() for ch in c) >= 2 and any(ch.isalpha() for ch in c):
            return c


def number(rng, n):
    first = rng.choice("123456789")
    return first + "".join(rng.choice(string.digits) for _ in range(n - 1))


def ref(rng):
    chars = string.ascii_uppercase + string.digits
    while True:
        c = "".join(rng.choice(chars) for _ in range(rng.choice([6, 7, 8, 10])))
        if any(ch.isdigit() for ch in c):
            return c


def phone(rng):
    return rng.choice([
        f"1-800-{number(rng, 3)}-{number(rng, 4)}",
        f"+1 {number(rng, 3)} {number(rng, 3)} {number(rng, 4)}",
        f"({number(rng, 3)}) {number(rng, 3)}-{number(rng, 4)}",
        f"+44 20 {number(rng, 4)} {number(rng, 4)}",
        f"0{number(rng, 3)} {number(rng, 3)} {number(rng, 4)}",
    ])


def amount(rng):
    v = f"{rng.randint(1, 9999)}.{rng.randint(0, 99):02d}"
    return rng.choice([f"${v}", f"€{v}", f"£{v}", f"{v} USD", f"USD {v}"])


def date(rng):
    return rng.choice([
        f"{rng.randint(1, 28)}/{rng.randint(1, 12)}/{rng.choice([2024, 2025, 2026])}",
        f"{rng.choice(['Jan', 'Mar', 'Jun', 'Sep', 'Nov'])} {rng.randint(1, 28)}, {rng.choice([2024, 2025, 2026])}",
        f"{rng.choice([2024, 2025, 2026])}-{rng.randint(1, 12):02d}-{rng.randint(1, 28):02d}",
    ])


# Sign-in and verification messages: {code} is the code; other fields are
# filled in. (subject, body) pairs.
POSITIVE = [
    ("Your {svc} verification code", "Hi {name},\n\nYour verification code is {code}.\n\nThis code expires in {min} minutes. If you didn't request it, you can ignore this email.\n\nThanks,\nThe {svc} team"),
    ("{code} is your {svc} verification code", "{code} is your {svc} verification code. Don't share it with anyone."),
    ("Sign in to {svc}", "Use this code to sign in to {svc}:\n\n{code}\n\nThe code is valid for {min} minutes.\n\nIf this wasn't you, change your password."),
    ("Your one-time passcode", "Your one-time passcode (OTP) is {code}. It will expire in {min} minutes. Never share this passcode."),
    ("{svc} security code", "Security code: {code}\n\nWe received a request to sign in to your {svc} account. Enter this code to continue."),
    ("Verify your email address", "Thanks for signing up for {svc}! To finish creating your account, enter this verification code:\n{code}\nThe code expires in {min} minutes."),
    ("Your login code", "Here is your login code: {code}\n\nIf you did not try to log in, please secure your account."),
    ("Two-factor authentication code", "Your two-factor authentication code is {code}. Enter it on the login page to continue."),
    ("{svc}: confirm it's you", "Hi {name},\nSomeone is trying to sign in to your account from a new device. If it's you, enter the code below.\n\n{code}\n\n— {svc} Security"),
    ("Your {svc} code", "{code}\nEnter this code in {svc} to verify your identity. It expires soon."),
    ("Password reset code", "We received a request to reset your password. Your reset code is {code}. If you didn't ask for this, ignore this message."),
    ("Account verification", "Dear customer,\n\nPlease use the following One Time Password (OTP) to complete your transaction: {code}\n\nDo not disclose this OTP to anyone. Valid for {min} minutes.\n\nRegards,\n{svc}"),
    ("New sign-in to your account", "To complete your sign-in, enter the following code: {code}\nRequest from {city_short}, {date}.\nIf this wasn't you, contact {phone}."),
    ("Confirm your login", "Your confirmation code: {code}\n\nThis code was requested to log in to {svc} at {date}."),
    ("{svc} 2FA", "2FA code: {code}"),
    ("Your access code", "Your access code is {code}. Use it within {min} minutes to access your account."),
    ("Email verification code", "Enter {code} to verify your email address with {svc}."),
    ("Your {svc} authentication code", "Authentication code\n\n{code}\n\nEnter this code to finish signing in. For your security, the code expires in {min} minutes.\n\n{svc}, {street}, {city}"),
    ("Complete your sign in", "Hello {name},\nYou recently tried to sign in. Copy and paste this temporary code: {code}\nIf you didn't request this code, you can safely ignore this email.\n© 2026 {svc}"),
    ("Magic code", "Your magic code is {code}. Paste it into the browser window where you started signing in."),
    ("Verification required", "A verification is required for your recent request. Code: {code}. Expires in {min} min."),
    ("Single-use code", "Hi {name},\n\nWe received your request for a single-use code to use with your {svc} account.\n\nYour single-use code is: {code}\n\nIf you didn't request this code, you can safely ignore this email.\n\nThanks,\nThe {svc} account team"),
    ("Your {svc} PIN", "Your temporary PIN is {code}. Enter it to unlock your account."),
    ("Device confirmation", "Confirm the new device with this code: {code}. Device: Chrome on Windows, {date}."),
    ("Verify it's you", "We noticed a new login to your account. Verification code\n{code}\nThis code expires in {min} minutes. Need help? Call {phone}."),
    ("Código de verificación", "Tu código de verificación de {svc} es {code}. Caduca en {min} minutos."),
    ("Dein Bestätigungscode", "Dein {svc} Bestätigungscode lautet {code}. Er ist {min} Minuten gültig."),
    ("Votre code de vérification", "Votre code de vérification {svc} est {code}. Il expire dans {min} minutes."),
    ("Seu código de acesso", "Seu código de acesso é {code}. Não compartilhe este código."),
    ("Il tuo codice di accesso", "Il tuo codice di accesso a {svc} è {code}. Scade tra {min} minuti."),
    ("Je verificatiecode", "Je {svc} verificatiecode is {code}. Deze code verloopt over {min} minuten."),
    ("Your sign-in code", "Sign-in code: {code}\nOr click the button below to sign in.\nIf you didn't try to sign in, someone may have your password."),
    ("Action required: verify your identity", "To keep your account secure, enter the security code {code} when prompted.\nOrder history and other details are unaffected."),
    ("{svc} login verification", "Login verification\nCode: {code}\nIP address: 203.0.113.{ip}\nTime: {date}"),
    ("Your code is ready", "Hi {name}, enter {code} in the app to continue. The code works once."),
    ("Your verification code", "<b>{code}</b> is your code"),
    ("Confirm your email", "Welcome to {svc}!\nYour confirmation code is {code}\nEnter it in the app to activate your account."),
    ("Here's your code", "Your {svc} code: {code}\n\nDidn't request a code? Reset your password."),
    ("Your {svc} passcode", "Passcode: {code}\nUse this passcode to finish logging in. It can only be used once."),
    ("Approve sign-in request", "Enter the number {code} in the {svc} app to approve this sign-in request from {city_short}."),
    ("Verification", "{code} - use this code to verify your {svc} account."),
    ("Your {svc} OTP", "OTP for login is {code}. Valid till {min} mins. Do not share it with anyone, including bank staff."),
    ("Recovery code", "Your account recovery code is {code}. Enter it to regain access to {svc}."),
    ("Confirm your phone number", "Your {svc} confirmation code is {code}. Enter it to confirm your phone number."),
    ("Código de acceso", "Hola {name}, usa el código {code} para iniciar sesión en {svc}. Válido durante {min} minutos."),
    ("Tu código de inicio de sesión", "Introduce este código para iniciar sesión: {code}\nSi no fuiste tú, ignora este correo."),
    ("Anmeldecode für {svc}", "Ihr Anmeldecode: {code}\nGeben Sie diesen Code ein, um die Anmeldung abzuschließen."),
    ("Ihr Sicherheitscode", "Ihr Sicherheitscode lautet {code}. Teilen Sie ihn mit niemandem."),
    ("Code de connexion", "Utilisez le code {code} pour vous connecter à {svc}. Ce code expire dans {min} minutes."),
    ("Votre code de sécurité", "Code de sécurité : {code}\nNe le communiquez à personne."),
    ("Código de verificação", "Seu código de verificação {svc} é {code}. Ele expira em {min} minutos."),
    ("Codice di verifica", "Il tuo codice di verifica è {code}. Non condividerlo con nessuno."),
    ("Inlogcode", "Gebruik code {code} om in te loggen bij {svc}."),
    ("Din verifieringskod", "Din verifieringskod för {svc} är {code}."),
    ("Kod weryfikacyjny", "Twój kod weryfikacyjny {svc} to {code}. Kod wygaśnie za {min} minut."),
]

# Mail with numbers that are not sign-in codes. {n} fields are filled with
# assorted numbers.
NEGATIVE = [
    ("Your order #{num6} has shipped", "Hi {name},\nGood news! Your order #{num6} is on its way. Tracking number: {ref}\nEstimated delivery: {date}.\nTotal: {amount}"),
    ("Invoice {ref}", "Invoice number: {ref}\nAmount due: {amount}\nDue date: {date}\nAccount: {num8}\n{svc}, {street}, {city}"),
    ("Payment received", "We received your payment of {amount} on {date}. Reference: {num7}. Thank you for your business."),
    ("Your receipt from {svc}", "Receipt #{num4}-{num4}\nDate: {date}\nPaid with Visa ending in {num4}\nTotal {amount}"),
    ("Meeting tomorrow", "Hi {name}, are we still on for tomorrow at 10:30? Room {num4}, building 2. Call me at {phone} if anything changes."),
    ("Your statement is ready", "Your statement for account ending in {num4} is ready. Balance: {amount}. Minimum payment due {date}."),
    ("Booking confirmation", "Your flight booking is confirmed. Booking reference: {ref}\nFlight {air}{num4}, {date}\nSeat 14C\nFrequent flyer number: {num8}"),
    ("Reservation {ref} confirmed", "Thank you for your reservation at {svc}. Confirmation number: {num8}. Check-in: {date}. Address: {street}, {city}."),
    ("Weekly newsletter", "Top stories this week:\n1. {svc} launches new features\n2. Prices start at {amount}\nRead more at our blog. Unsubscribe anytime. © 2026 {svc}, {street}, {city}"),
    ("Support ticket {num6}", "Hi {name}, your ticket #{num6} has been updated. Reply to this email to add comments. Case ID: {ref}."),
    ("Your package was delivered", "Your package with tracking number {num10} was delivered on {date} at 2:45 PM."),
    ("Join our webinar", "Join us on {date} at 18:00. Zoom meeting ID: {num3} {num4} {num4}. Passcode: {num6}. Call-in: {phone}."),
    ("Your subscription renews soon", "Your {svc} subscription (customer ID {num7}) renews on {date} for {amount}."),
    ("Shipping update", "Order {ref} - {num3} items shipped from warehouse {num4}. Expected {date}."),
    ("Lunch?", "Hey {name}, lunch at noon? I'm at {street}. My new number is {phone}."),
    ("Your {svc} account summary", "Account number: {num10}\nRouting number: {num9}\nAvailable balance: {amount}\nStatement period ending {date}."),
    ("Tax document available", "Your 1099 for tax year {year} is available. Taxpayer ID ending {num4}."),
    ("Event tickets", "Your tickets for the concert on {date}. Order {num7}. Section 104, Row 12, Seats 7-8. Ticket barcode: {num10}."),
    ("Product recall notice", "Models {ref} and {ref2} manufactured in {year} are affected. Serial numbers {num6} through {num6b}."),
    ("Your refund is on its way", "We issued a refund of {amount} to your card ending in {num4}. Refund ID: {ref}. Allow 5-10 business days."),
    ("Delivery attempt failed", "We tried to deliver parcel {ref} to {street}, {city} on {date}. Call {phone} to reschedule."),
    ("Your {svc} order confirmation", "Order number: {num8}\nItem: Wireless Mouse (SKU {ref})\nQty 1\nSubtotal {amount}\nShipping to {city}"),
    ("Pedido {num6} enviado", "Tu pedido {num6} ha sido enviado. Número de seguimiento: {ref}. Total: {amount}."),
    ("Ihre Rechnung {num7}", "Rechnungsnummer: {num7}\nBetrag: {amount}\nKundennummer: {num6}"),
    ("Votre commande {num6}", "Votre commande n° {num6} a été expédiée. Suivi: {ref}."),
    ("Happy birthday!", "Happy birthday {name}! Use promo code {promo} for {pct}% off your next order, valid until {date}."),
    ("Your coupon", "Here's {pct}% off: enter coupon code {promo} at checkout. Expires {date}."),
    ("Gift card", "You received a {svc} gift card worth {amount}. Gift card number: {num8} PIN: {num4}."),
    ("Security alert", "We noticed a new sign-in to your {svc} account from {city_short} on {date}. If this was you, no action is needed. Account ID {num7}."),
    ("Password changed", "The password for your {svc} account was changed on {date}. If you didn't do this, call {phone}."),
    ("Your login details", "Your username: {name}{num4}\nCustomer number: {num8}\nVisit {svc} to sign in."),
    ("Build {num4} failed", "Pipeline #{num5} failed on commit {hex7}. Job {num6} exited with code 1."),
    ("Pull request #{num4}", "{name} opened pull request #{num4}: Fix overflow in parser. Merging {hex7} into main."),
    ("Your appointment", "Appointment confirmed for {date} at 9:15 AM with Dr. {name}. Patient ID: {num7}. Location: {street}, {city}."),
    ("Bill reminder", "Your electricity bill of {amount} for account {num10} is due on {date}. Meter {ref}."),
    ("Flash sale", "Today only: use discount code {promo} to save {pct}% on orders over {amount}. Shop now!"),
    ("Your referral code", "Share your referral code {promo} with friends. You both get {amount} credit."),
    ("Exclusive offer for you", "Get {pct}% off with voucher {promo}. Offer ends {date}. Terms apply."),
    ("Código promocional", "Usa el código promocional {promo} y ahorra un {pct}% hasta el {date}."),
    ("Gutscheincode", "Mit dem Gutscheincode {promo} sparen Sie {pct}% bis zum {date}."),
    ("Your Wi-Fi details", "Network: Guest-{num4}\nPassword: {ref}\nRoom {num3}"),
    ("Order {num7} ready for pickup", "Your order {num7} is ready for pickup at store {num4}. Bring your ID and order number."),
    ("Confirmación de reserva", "Su reserva {ref} está confirmada para el {date}. Número de cliente: {num7}."),
    ("Ihre Bestellung {num8}", "Vielen Dank für Ihre Bestellung {num8}. Lieferung am {date}. Sendungsnummer {num10}."),
    ("Votre facture", "Facture n° {num7} du {date}. Montant : {amount}. Client {num6}."),
    ("Version {num4} released", "Release notes for build {num5}: fixed {num3} issues. Download size {num4} KB."),
    ("Your parking permit", "Permit number {ref} for vehicle plate {ref2} is valid until {date}."),
    ("Conference registration", "Registration ID: {num6}. Badge pickup at Hall {num4}. Event code {promo}."),
]

AIR = ["BA", "LH", "AF", "UA", "DL", "IB", "KL"]


def fill(rng, templates, the_code):
    """The templates filled in with one set of values."""
    values = {
        "svc": rng.choice(SERVICES),
        "name": rng.choice(NAMES),
        "code": the_code,
        "min": rng.choice(MINUTES),
        "city": rng.choice(CITIES),
        "city_short": rng.choice(CITIES).split(",")[0].split(" ")[0],
        "street": rng.choice(STREETS),
        "phone": phone(rng),
        "date": date(rng),
        "amount": amount(rng),
        "ref": ref(rng),
        "ref2": ref(rng),
        "num3": number(rng, 3),
        "num4": number(rng, 4),
        "num5": number(rng, 5),
        "num6": number(rng, 6),
        "num6b": number(rng, 6),
        "num7": number(rng, 7),
        "num8": number(rng, 8),
        "num9": number(rng, 9),
        "num10": number(rng, 10),
        "year": str(rng.choice([2023, 2024, 2025, 2026])),
        "ip": str(rng.randint(1, 254)),
        "air": rng.choice(AIR),
        "promo": rng.choice(["SAVE20", "WELCOME10", "SPRING25", "VIP2026", "BDAY15"]),
        "pct": str(rng.choice([10, 15, 20, 25])),
        "hex7": "".join(rng.choice("0123456789abcdef") for _ in range(7)),
    }
    return tuple(t.format(**values) for t in templates)


def vary(rng, subject, body):
    """Small changes in layout: extra greeting, footer, spacing, case."""
    if rng.random() < 0.3:
        body = rng.choice(["Hello,\n\n", "Hi there,\n", "Dear user,\n\n", ""]) + body
    if rng.random() < 0.4:
        body += rng.choice([
            "\n\nThis is an automated message, please do not reply.",
            "\n\nNeed help? Visit our help center or call " + phone(rng) + ".",
            "\n\n© " + str(rng.choice([2024, 2025, 2026])) + " All rights reserved. " + rng.choice(STREETS) + ", " + rng.choice(CITIES),
            "\n\nUnsubscribe | Privacy | Terms",
            "\n\nSent from " + rng.choice(STREETS),
        ])
    if rng.random() < 0.1:
        subject = subject.upper()
    if rng.random() < 0.2:
        body = body.replace("\n\n", "\n")
    return subject, body


def split(templates, seed=7):
    """Two disjoint halves of `templates`: (train, held out)."""
    rng = random.Random(seed)
    idx = list(range(len(templates)))
    rng.shuffle(idx)
    cut = int(len(idx) * 0.75)
    return [templates[i] for i in idx[:cut]], [templates[i] for i in idx[cut:]]


def messages(rng, positive, negative, count):
    """(subject, body, code or None) triples, about half with a code."""
    out = []
    for _ in range(count):
        if rng.random() < 0.5:
            c = code(rng)
            out.append((*vary(rng, *fill(rng, rng.choice(positive), c)), c))
        else:
            out.append((*vary(rng, *fill(rng, rng.choice(negative), "")), None))
    return out
