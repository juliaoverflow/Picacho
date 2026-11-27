"""Made-up mail for training the inbox category model, labelled as it is
written.

Each message is what a message list holds of it: the sender's name and
address, a subject, a preview, and its list headers (List-Unsubscribe,
List-Id, List-Post). Nothing in it is real.

- Primary is people's mail: the reply model's messages
  (../replies/languages.py, 38 languages) from people's addresses, some
  of them at work or from a role address ("hello@" a small studio), a few
  forwarded or replied to.
- Transactions, Travel, Finance, Updates, Newsletters, Promotions and
  Social are put together from templates_a.py to templates_c.py (English,
  Spanish, German, French, Portuguese, Italian, Dutch, Polish: two or more
  wordings of each kind of message) and templates_d.py to templates_i.py
  (the reply model's 30 other languages: one wording of each), with
  made-up and real shops, airlines, banks, publications,
  services, social networks and discussion groups, their senders' addresses
  (subdomains such as "e." and "mail.", "noreply@" and the like), list
  headers as such mail has them (marketing services' unsubscribe links), a
  preheader or footer, and slips (lowercase, no preview).
- Forums also takes people's messages from the reply model, sent through a
  discussion list.

Targets are distributions over CLASSES: one category, or for mail that
could be either (a shop's newsletter without an offer, a welcome mail), a
share for the other. `half` picks the alternatives used (0: even-numbered,
1: odd), so the held-out half measures wording the model never saw; in
the languages with one wording of each kind, it picks the kinds instead
(`kinds_for`), so the held-out half is kinds of mail never seen in them.
"""

import importlib.util
import random
import sys
from pathlib import Path

from templates_a import T as TA
from templates_b import T as TB
from templates_c import T as TC
from templates_d import L as LD
from templates_e import L as LE
from templates_f import L as LF
from templates_g import L as LG
from templates_h import L as LH
from templates_i import L as LI

HERE = Path(__file__).parent
CLASSES = ["primary", "transactions", "travel", "finance", "updates", "newsletters", "promotions", "social", "forums"]
INDEX = {c: i for i, c in enumerate(CLASSES)}


def _replies():
    """The reply model's data module (people's mail in 38 languages)."""
    sys.path.insert(0, str(HERE.parent / "replies"))
    spec = importlib.util.spec_from_file_location("replies_data", HERE.parent / "replies" / "data.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    sys.path.pop(0)
    return module


REPLIES = _replies()
days = sys.modules["languages"].days
REPLY_LANGUAGES = REPLIES.LANGUAGES

TEMPLATES = {}
for part in (TA, TB, TC):
    for kind, by_lang in part.items():
        TEMPLATES.setdefault(kind, {}).update(by_lang)
# The languages with two or more wordings of each kind of message, and the
# others, with one each (and their own products, topics, preheader, footer
# and currency).
WORDED = ["en", "es", "de", "fr", "pt", "it", "nl", "pl"]
OTHER_LANGUAGES = {}
for part in (LD, LE, LF, LG, LH, LI):
    OTHER_LANGUAGES.update(part)
for lang, entries in OTHER_LANGUAGES.items():
    for kind, text in entries.items():
        if not kind.startswith("_"):
            TEMPLATES.setdefault(kind, {})[lang] = [text]
# Every language the other models have (the reply model's), as they are.
LANGUAGES = list(REPLY_LANGUAGES)
LANGUAGE_WEIGHT = {"en": 4, **{l: 1.5 for l in WORDED if l != "en"}}

KINDS = {
    "transactions": ["order", "shipped", "delivered", "receipt", "refund"],
    "travel": ["booking", "flight"],
    "finance": ["statement", "bill", "banking", "account", "money"],
    "updates": ["security", "reminder", "renewal", "notification", "welcome", "policy"],
    "newsletters": ["newsletter", "issue"],
    "promotions": ["sale", "code", "arrivals", "freeship", "loyalty", "event", "comeback"],
    "social": ["comment", "follow", "message", "friend", "mention", "memories", "like"],
    "forums": ["post", "digest", "topic"],
}
# Mail that could be counted as another category too.
SOFT = {
    "newsletter": {"newsletters": 0.8, "updates": 0.1, "promotions": 0.1},
    "welcome": {"updates": 0.8, "promotions": 0.2},
    "event": {"promotions": 0.85, "updates": 0.15},
    "memories": {"social": 0.85, "updates": 0.15},
    "booking": {"travel": 0.9, "transactions": 0.1},
    "bill": {"finance": 0.85, "updates": 0.15},
    "banking": {"finance": 0.75, "transactions": 0.25},
    "money": {"finance": 0.8, "transactions": 0.2},
    "renewal": {"updates": 0.8, "transactions": 0.2},
}
SHARE = {"primary": 0.26, "transactions": 0.10, "travel": 0.06, "finance": 0.09, "updates": 0.13,
         "newsletters": 0.08, "promotions": 0.14, "social": 0.08, "forums": 0.06}
# Real names among the made-up ones, by category.
REAL_BY_CATEGORY = {
    "travel": ["Booking.com", "Airbnb", "Delta", "Lufthansa", "Ryanair", "Iberia", "Expedia", "KLM",
               "Air France", "Trainline", "SNCF", "Marriott", "Hilton", "easyJet"],
    "finance": ["PayPal", "Chase", "Revolut", "N26", "ING", "Santander", "BBVA", "Wise", "Monzo",
                "Barclays", "HSBC", "Capital One", "American Express", "Nubank"],
    "newsletters": ["Morning Brew", "The Hustle", "Stratechery", "Platformer", "The Browser",
                    "Dense Discovery", "Benedict's Newsletter"],
}
SYLLABLES_TRAVEL = ["Air", "Jet", "Sky", "Rail", "Stay", "Trip", "Voya", "Wander", "Atlas", "Horizon"]
SYLLABLES_FINANCE = ["Bank", "Pay", "Capital", "Trust", "Credit", "Fin", "Coin", "Ledger", "Union", "Mutual"]
PUBLICATIONS = ["The Weekly Byte", "Morning Notes", "The Long Read", "Field Notes", "The Margin",
                "Signal & Noise", "Sunday Letters", "The Quiet Week", "Tech Brief", "Kitchen Table"]
NEWSLETTER_HOSTS = ["substack.com", "beehiiv.com", "buttondown.email", "ghost.io"]

FIRST = ["Ana", "Luis", "Sam", "Mei", "Ravi", "Olga", "Kofi", "Yuki", "Fatima", "Lars", "Ingrid", "Marta",
         "Tomás", "Priya", "Jonas", "Camille", "Giulia", "João", "Sven", "Wei", "Minji", "Ivan", "Ayşe",
         "Noah", "Emma", "Lukas", "Chloé", "Mateo", "Zofia", "Pieter", "Hanna", "Diego", "Amara", "Leo"]
LAST = ["Silva", "García", "Müller", "Martin", "Rossi", "de Vries", "Kowalski", "Smith", "Chen", "Patel",
        "Nowak", "Dubois", "Santos", "Novak", "Jensen", "Kim", "Okafor", "Haddad", "Ivanova", "Larsen",
        "Moreau", "Bianchi", "Jansen", "Wójcik", "Ferreira", "Schmidt", "López", "Brown", "Tanaka"]
PERSONAL_DOMAINS = ["gmail.com", "outlook.com", "yahoo.com", "icloud.com", "hotmail.com", "proton.me",
                    "gmx.de", "web.de", "orange.fr", "free.fr", "libero.it", "uol.com.br", "wp.pl",
                    "ziggo.nl", "yandex.ru", "mail.ru", "fastmail.com", "hey.com"]
WORK_DOMAINS = ["acme-corp.com", "northwind.io", "studio-luma.design", "kestrel.co", "brightline.org",
                "mail.university.edu", "email.arizona.edu", "cs.uni-bonn.de", "hospital-sur.es", "lyon-avocats.fr"]
ROLE_LOCALS = ["hello", "info", "contact", "office", "team", "studio"]

SYLLABLES_A = ["Lu", "Ka", "Mo", "Ve", "Zen", "Tri", "Bri", "Ar", "No", "Sol", "Fin", "Cor", "Del", "Pix",
               "Quo", "Ro", "Sta", "Ul", "Wi", "Ya", "Ever", "Blue", "North", "Oak", "Peak", "Silver"]
SYLLABLES_B = ["ra", "vo", "lix", "ton", "ly", "ify", "dex", "mo", "nova", "stack", "bay", "mart", "wear",
               "home", "go", "pay", "bank", "air", "fit", "hub", "lane", "field", "works", "box"]
REAL_BRANDS = ["Amazon", "IKEA", "Zara", "Nike", "Delta", "Spotify", "Netflix", "Uber", "Airbnb", "PayPal",
               "Chase", "Booking.com", "Etsy", "Decathlon", "Lidl", "Zalando", "MediaMarkt", "Fnac", "Notion",
               "GitHub", "Dropbox", "Revolut", "N26", "Vodafone"]
TLDS = ["com", "com", "com", "co", "io", "de", "fr", "es", "it", "nl", "pl", "com.br", "co.uk", "app", "shop"]

REAL_SOCIAL = [("Facebook", "facebookmail.com"), ("Instagram", "mail.instagram.com"), ("LinkedIn", "linkedin.com"),
               ("X", "x.com"), ("TikTok", "tiktok.com"), ("Pinterest", "pinterest.com"), ("Reddit", "redditmail.com"),
               ("Nextdoor", "nextdoor.com"), ("Strava", "strava.com"), ("Bluesky", "bsky.app"),
               ("Discord", "discord.com"), ("Tinder", "gotinder.com"), ("Bumble", "bumble.com"),
               ("Meetup", "meetup.com"), ("Goodreads", "goodreads.com"), ("YouTube", "youtube.com")]
MADE_UP_SOCIAL = ["Circlo", "Snapverse", "Kinnect", "Pallo", "Vibely", "Hoots", "Glimpse", "Tribely", "Yapster",
                  "Mingle", "Huddl", "Nookly"]
SOCIAL_LOCALS = ["notification", "notifications", "no-reply", "noreply", "friends", "update", "info", "messages",
                 "notify", "member", "updates", "security"]

LOCALS = {
    "transactions": ["orders", "order-update", "auto-confirm", "shipment-tracking", "receipts", "noreply", "no-reply",
                     "payments", "billing", "store", "shop", "service", "info", "support", "bookings",
                     "reservations", "tickets", "confirmation", "ship-confirm", "transaction"],
    "updates": ["noreply", "no-reply", "notifications", "alerts", "account", "security", "info", "hello", "news",
                "newsletter", "team", "support", "billing", "updates", "do-not-reply", "calendar", "mailer",
                "digest", "editor", "weekly"],
    "promotions": ["news", "hello", "offers", "deals", "marketing", "newsletter", "info", "shop", "store", "team",
                   "promo", "rewards", "vip", "email", "hi", "contact", "events", "community"],
    "travel": ["reservations", "bookings", "noreply", "no-reply", "travel", "flights", "itinerary", "checkin",
               "confirmation", "info", "customer-service", "trips", "service", "notifications"],
    "finance": ["alerts", "noreply", "no-reply", "statements", "billing", "notifications", "service",
                "onlinebanking", "customerservice", "info", "payments", "account", "security"],
    "newsletters": ["newsletter", "news", "digest", "editor", "weekly", "hello", "daily", "briefing", "team",
                    "letters", "noreply", "dispatch"],
}
SUBDOMAINS = {
    "transactions": ["", "", "", "", "mail.", "orders.", "email.", "e.", "notifications."],
    "updates": ["", "", "", "mail.", "notifications.", "alerts.", "accounts.", "email.", "news."],
    "promotions": ["", "", "e.", "email.", "mail.", "em.", "news.", "info.", "go.", "m.", "mkt."],
    "travel": ["", "", "", "mail.", "email.", "e.", "reservations.", "notifications."],
    "finance": ["", "", "", "alerts.", "ealerts.", "mail.", "notifications.", "email."],
    "newsletters": ["", "", "", "news.", "mail.", "email.", "letters."],
}
# How often each kind of mail has each list header.
HEADERS = {  # unsubscribe, list-id, list-post, through a marketing service
    "transactions": (0.35, 0.05, 0.0, 0.05),
    "updates": (0.55, 0.25, 0.02, 0.10),
    "promotions": (0.97, 0.40, 0.01, 0.55),
    "social": (0.70, 0.20, 0.0, 0.05),
    "travel": (0.40, 0.05, 0.0, 0.05),
    "finance": (0.30, 0.03, 0.0, 0.02),
    "newsletters": (0.98, 0.60, 0.02, 0.35),
}
SERVICE_LINKS = ["https://{b}.us{n}.list-manage.com/unsubscribe?u=a1b2&id=c3d4", "https://manage.kmail-lists.com/klclick/{b}",
                 "https://click.e.{d}/unsub_center.aspx?qs=exct.net", "https://{b}.hs-sites.com/hubspotemail/unsub",
                 "https://visitor.constantcontact.com/do?p=un&m={b}", "https://{b}.createsend1.com/t/unsubscribe",
                 "https://r.sendinblue.com/{b}/unsub", "https://{b}.omnisend.com/unsubscribe",
                 "https://link.iterable.com/u/{b}", "https://{b}.braze.com/unsubscribe"]

ITEMS = {
    "en": ["running shoes", "wireless headphones", "coffee maker", "linen shirt", "backpack", "desk lamp"],
    "es": ["zapatillas", "auriculares", "cafetera", "camisa de lino", "mochila", "lámpara"],
    "de": ["Laufschuhe", "Kopfhörer", "Kaffeemaschine", "Leinenhemd", "Rucksack", "Schreibtischlampe"],
    "fr": ["baskets", "écouteurs", "cafetière", "chemise en lin", "sac à dos", "lampe de bureau"],
    "pt": ["tênis", "fones de ouvido", "cafeteira", "camisa de linho", "mochila", "luminária"],
    "it": ["scarpe da corsa", "cuffie", "macchina da caffè", "camicia di lino", "zaino", "lampada"],
    "nl": ["hardloopschoenen", "koptelefoon", "koffiezetapparaat", "linnen overhemd", "rugzak", "bureaulamp"],
    "pl": ["buty do biegania", "słuchawki", "ekspres do kawy", "lniana koszula", "plecak", "lampka"],
}
TOPICS = {
    "en": ["product design", "climate", "remote work", "the Q3 roadmap", "AI", "the budget", "release 2.4", "home cooking"],
    "es": ["diseño de producto", "el clima", "teletrabajo", "el presupuesto", "la versión 2.4", "cocina"],
    "de": ["Produktdesign", "Klima", "Homeoffice", "das Budget", "Version 2.4", "Kochen"],
    "fr": ["design produit", "le climat", "télétravail", "le budget", "la version 2.4", "cuisine"],
    "pt": ["design de produto", "clima", "trabalho remoto", "o orçamento", "a versão 2.4", "culinária"],
    "it": ["design di prodotto", "clima", "lavoro da remoto", "il budget", "la versione 2.4", "cucina"],
    "nl": ["productontwerp", "klimaat", "thuiswerken", "het budget", "versie 2.4", "koken"],
    "pl": ["projektowanie produktu", "klimat", "praca zdalna", "budżet", "wersja 2.4", "gotowanie"],
}
CITIES = ["Lisbon", "Berlin", "Madrid", "Paris", "Rome", "Amsterdam", "Warsaw", "New York", "Tokyo", "São Paulo"]
GROUPS = ["rust-users", "photo-club", "hoa-board", "ml-reading", "linux-dev", "bike-commuters", "parents-network",
          "jazz-fans", "python-es", "makers-berlin", "astro-lisboa", "chess-club"]
GROUP_HOSTS = ["googlegroups.com", "lists.{d}", "groups.io", "freelists.org", "discoursemail.com", "lists.sourceforge.net"]

PREHEADERS = {
    "en": ["View this email in your browser", "Having trouble viewing this email? View it online", "{brand} |"],
    "es": ["Ver este correo en el navegador", "¿No ves bien este correo? Ábrelo en tu navegador"],
    "de": ["E-Mail im Browser anzeigen", "Wird diese E-Mail nicht richtig angezeigt?"],
    "fr": ["Voir cet e-mail dans votre navigateur", "Vous ne voyez pas bien ce message ?"],
    "pt": ["Ver este e-mail no navegador", "Não consegue ver este e-mail?"],
    "it": ["Visualizza questa e-mail nel browser", "Non visualizzi correttamente questa e-mail?"],
    "nl": ["Bekijk deze e-mail in je browser", "Wordt deze e-mail niet goed weergegeven?"],
    "pl": ["Zobacz tę wiadomość w przeglądarce", "Wiadomość nie wyświetla się poprawnie?"],
}
FOOTERS = {
    "en": ["Unsubscribe · Privacy policy · © 2026 {brand}", "You're receiving this because you signed up at {brand}."],
    "es": ["Darse de baja · Privacidad · © 2026 {brand}", "Recibes este correo porque te registraste en {brand}."],
    "de": ["Abmelden · Datenschutz · © 2026 {brand}", "Sie erhalten diese E-Mail, weil Sie sich bei {brand} angemeldet haben."],
    "fr": ["Se désinscrire · Confidentialité · © 2026 {brand}", "Vous recevez cet e-mail car vous êtes inscrit chez {brand}."],
    "pt": ["Cancelar inscrição · Privacidade · © 2026 {brand}", "Você recebe este e-mail porque se cadastrou na {brand}."],
    "it": ["Annulla iscrizione · Privacy · © 2026 {brand}", "Ricevi questa e-mail perché sei iscritto a {brand}."],
    "nl": ["Afmelden · Privacy · © 2026 {brand}", "Je ontvangt deze e-mail omdat je je hebt aangemeld bij {brand}."],
    "pl": ["Wypisz się · Prywatność · © 2026 {brand}", "Otrzymujesz tę wiadomość, ponieważ zarejestrowałeś się w {brand}."],
}


def ascii_name(s):
    table = str.maketrans("áàâãäçéèêëíìîïñóòôõöúùûüýşğıłńśźżćąęøåæß", "aaaaaceeeeiiiinooooouuuuysgilnszzcaeoaas")
    return "".join(c for c in s.lower().translate(table) if c.isascii() and (c.isalnum() or c in ".-"))


def brand_name(rng, category=None):
    if category in REAL_BY_CATEGORY and rng.random() < 0.4:
        return rng.choice(REAL_BY_CATEGORY[category])
    if category == "travel" and rng.random() < 0.6:
        return rng.choice(SYLLABLES_TRAVEL) + rng.choice(["line", "go", "way", "air", "stay", "trip", "rail"])
    if category == "finance" and rng.random() < 0.6:
        return rng.choice(SYLLABLES_FINANCE) + rng.choice(["ly", "ex", "first", "one", "wise", "bank", ""])
    if category == "newsletters" and rng.random() < 0.6:
        return rng.choice(PUBLICATIONS)
    if rng.random() < 0.3:
        return rng.choice(REAL_BRANDS)
    return rng.choice(SYLLABLES_A) + rng.choice(SYLLABLES_B)


def brand_domain(rng, brand):
    base = ascii_name(brand.replace(" ", "")).replace(".", "") or "brand"
    if brand == "Booking.com":
        return "booking.com"
    return f"{base}.{rng.choice(TLDS)}"


# Languages that write decimals with a point.
DECIMAL_POINT = {"en", "he", "hi", "bn", "th", "ja", "zh", "zh-hant", "ko", "tl", "ms", "sw"}


def price(rng, lang):
    amount = f"{rng.randint(3, 480)}{rng.choice(['.00', '.99', '.50', '.10'])}"
    if lang in OTHER_LANGUAGES:
        if lang not in DECIMAL_POINT:
            amount = amount.replace(".", ",")
        if lang in ("ja", "ko", "vi", "id", "hu"):
            # Currencies without cents, in larger numbers.
            amount = f"{rng.randint(3, 480) * 100:,}".replace(",", "." if lang in ("vi", "id", "hu") else ",")
        return OTHER_LANGUAGES[lang]["_currency"].format(amount=amount)
    if lang == "en":
        return rng.choice([f"${amount}", f"£{amount}", f"€{amount}"])
    comma = amount.replace(".", ",")
    if lang == "pt":
        return rng.choice([f"R$ {comma}", f"€{comma}", f"{comma}€"])
    if lang == "pl":
        return rng.choice([f"{comma} zł", f"{comma} PLN", f"€{comma}"])
    return rng.choice([f"{comma} €", f"€{comma}", f"{comma}€"])


def person(rng):
    first, last = rng.choice(FIRST), rng.choice(LAST)
    r = rng.random()
    local = ascii_name(first)
    if r < 0.4:
        local = f"{ascii_name(first)}.{ascii_name(last).replace(' ', '')}"
    elif r < 0.6:
        local = f"{ascii_name(first)[0]}{ascii_name(last).replace(' ', '')}"
    elif r < 0.75:
        local = f"{ascii_name(first)}{rng.randint(1, 99)}"
    name = rng.choice([f"{first} {last}", f"{first} {last}", first, ""])
    return first, name, local


def items(lang):
    return ITEMS[lang] if lang in ITEMS else OTHER_LANGUAGES[lang]["_items"]


def topics(lang):
    return TOPICS[lang] if lang in TOPICS else OTHER_LANGUAGES[lang]["_topics"]


def preheaders(lang):
    return PREHEADERS[lang] if lang in PREHEADERS else [OTHER_LANGUAGES[lang]["_preheader"]]


def footers(lang):
    return FOOTERS[lang] if lang in FOOTERS else [OTHER_LANGUAGES[lang]["_footer"]]


def fill(text, rng, lang, brand, app):
    first = rng.choice(FIRST)
    values = {
        "{brand}": brand, "{app}": app, "{name}": first, "{group}": rng.choice(GROUPS),
        "{topic}": rng.choice(topics(lang)), "{item}": rng.choice(items(lang)), "{n}": str(rng.randint(2, 9)),
        "{pct}": str(rng.choice([10, 15, 20, 25, 30, 40, 50, 60, 70])), "{price}": price(rng, lang),
        "{order}": str(rng.randint(100000, 99999999)), "{day}": rng.choice(days(lang)),
        "{last4}": f"{rng.randint(0, 9999):04d}", "{city}": rng.choice(CITIES),
    }
    for k, v in values.items():
        text = text.replace(k, v)
    return text


def template(rng, kind, lang, half):
    alts = TEMPLATES[kind][lang]
    if half is not None:
        alts = [a for i, a in enumerate(alts) if i % 2 == half] or alts
    subject, preview = rng.choice(alts).split(" || ")
    return subject, preview


def unsubscribe(rng, domain, brand, service):
    b = ascii_name(brand.replace(" ", "")).replace(".", "") or "brand"
    if service:
        link = rng.choice(SERVICE_LINKS).format(b=b, d=domain, n=rng.randint(1, 21))
        return f"<{link}>" + (f", <mailto:unsub@{domain}>" if rng.random() < 0.4 else "")
    if rng.random() < 0.5:
        return f"<mailto:unsubscribe@{domain}?subject=unsubscribe>"
    return f"<https://{domain}/email/unsubscribe?u={rng.randint(1000, 99999)}>" + (
        f", <mailto:unsubscribe@{domain}>" if rng.random() < 0.5 else "")


def untidy(rng, subject, preview):
    if rng.random() < 0.08:
        subject, preview = subject.lower(), preview.lower()
    if rng.random() < 0.06:
        preview = ""
    return subject, preview


def from_person_text(rng, half):
    """People's mail from the reply model: (language, subject, preview)."""
    weights = [REPLIES.WEIGHT.get(lang, 1.5) if hasattr(REPLIES, "WEIGHT") else 1.5 for lang in REPLY_LANGUAGES]
    lang = rng.choices(REPLY_LANGUAGES, weights)[0]
    while True:
        subject, body, sit, _ = REPLIES.message(rng, lang, half if half is not None else rng.randrange(2))
        if sit != "automated":
            break
    preview = " ".join(body.split())[:200]
    return lang, subject, preview


def primary(rng, half):
    lang, subject, preview = from_person_text(rng, half)
    first, name, local = person(rng)
    r = rng.random()
    if r < 0.62:
        domain = rng.choice(PERSONAL_DOMAINS)
    elif r < 0.94:
        domain = rng.choice(WORK_DOMAINS)
    else:
        # A small business or freelancer writing from a role address.
        local, domain = rng.choice(ROLE_LOCALS), rng.choice(WORK_DOMAINS)
        name = rng.choice([f"{first} from {brand_name(rng)}", first, name])
    if rng.random() < 0.06:
        subject = "Fwd: " + subject
    unsub = list_id = ""
    if rng.random() < 0.01:
        unsub = f"<mailto:leave@{domain}>"
    return dict(lang=lang, name=name, address=f"{local}@{domain}", subject=subject, preview=preview,
                unsub=unsub, list_id=list_id, list_post=False, category="primary", kind="person",
                target={"primary": 1.0})


def kinds_for(category, lang, half):
    """The kinds of `category`'s mail a message may be. In the languages
    with one wording of each, there is no wording to hold out: `half` holds
    out kinds instead (even-numbered ones to train on, odd ones to test),
    so a held-out message is a kind of mail never seen in that language."""
    kinds = KINDS[category]
    if half is None or lang in WORDED or len(kinds) < 2:
        return kinds
    return [k for i, k in enumerate(kinds) if i % 2 == half]


def automated(rng, category, half):
    lang = rng.choices(LANGUAGES, [LANGUAGE_WEIGHT.get(l, 1) for l in LANGUAGES])[0]
    kind = rng.choice(kinds_for(category, lang, half))
    brand = brand_name(rng, category)
    app, domain, local = brand, None, None
    if category == "social":
        if rng.random() < 0.55:
            app, domain = rng.choice(REAL_SOCIAL)
        else:
            app = rng.choice(MADE_UP_SOCIAL)
            domain = f"{rng.choice(['', 'mail.', 'notifications.'])}{ascii_name(app)}.{rng.choice(['com', 'app', 'social', 'io'])}"
        local = rng.choice(SOCIAL_LOCALS)
        name = rng.choice([app, app, f"{app} Notifications", f"{rng.choice(FIRST)} via {app}", ""])
    else:
        # Senders overlap: a quarter of the time, another kind of mail's
        # address; now and then a small shop's personal address.
        pool = category if rng.random() >= 0.25 else rng.choice(list(LOCALS))
        domain = rng.choice(SUBDOMAINS[pool]) + brand_domain(rng, brand)
        local = rng.choice(LOCALS[pool])
        name = rng.choice([brand, brand, brand, f"{brand} Team", f"{brand} Support", f"{rng.choice(FIRST)} at {brand}", ""])
        if rng.random() < 0.04:
            local, domain = ascii_name(brand) or "shop", rng.choice(PERSONAL_DOMAINS)
        elif category == "newsletters" and rng.random() < 0.3:
            # A writer's own newsletter, through a newsletter service.
            local = ascii_name(rng.choice(FIRST)) or "writer"
            domain = rng.choice(NEWSLETTER_HOSTS)
            name = rng.choice([brand, rng.choice(FIRST) + " " + rng.choice(LAST), f"{brand} by {rng.choice(FIRST)}"])
    subject, preview = template(rng, kind, lang, half)
    subject = fill(subject, rng, lang, brand, app)
    preview = fill(preview, rng, lang, brand, app)
    if category in ("promotions", "updates", "transactions", "newsletters", "travel") and rng.random() < 0.3:
        preview = fill(rng.choice(preheaders(lang)), rng, lang, brand, app) + " " + preview
    if category in ("promotions", "updates", "social", "newsletters") and rng.random() < 0.25:
        preview = preview + " " + fill(rng.choice(footers(lang)), rng, lang, brand, app)
    p_unsub, p_list, p_post, p_service = HEADERS[category]
    service = category != "social" and rng.random() < p_service
    unsub = unsubscribe(rng, domain, brand, service) if rng.random() < p_unsub else ""
    list_id = ""
    if rng.random() < p_list:
        list_id = rng.choice([f"{rng.randint(10**5, 10**9):x}.list-id.mcsv.net", f"news.{domain}",
                              f"{ascii_name(rng.choice(TOPICS['en']))[:12] or 'news'}.{domain}"])
    subject, preview = untidy(rng, subject, preview)
    target = SOFT.get(kind, {category: 1.0})
    return dict(lang=lang, name=name, address=f"{local}@{domain}", subject=subject, preview=preview,
                unsub=unsub, list_id=list_id, list_post=rng.random() < p_post, category=category, kind=kind,
                target=target)


def forum(rng, half):
    group = rng.choice(GROUPS)
    host = rng.choice(GROUP_HOSTS).format(d=rng.choice(WORK_DOMAINS + ["example.org", "python.org", "debian.org"]))
    first, name, local = person(rng)
    r = rng.random()
    if r < 0.5:
        # A person's own words, sent through the list.
        lang, subject, preview = from_person_text(rng, half)
        if rng.random() < 0.7:
            subject = f"[{group}] {subject}" if rng.random() < 0.6 else f"Re: [{group}] {subject}"
        kind = "person"
    else:
        lang = rng.choices(LANGUAGES, [LANGUAGE_WEIGHT.get(l, 1) for l in LANGUAGES])[0]
        kind = rng.choice(kinds_for("forums", lang, half))
        subject, preview = template(rng, kind, lang, half)
        subject = fill(subject.replace("{group}", group), rng, lang, group, group)
        preview = fill(preview.replace("{group}", group), rng, lang, group, group)
    if rng.random() < 0.35:
        address = f"{group}@{host}"
        name = rng.choice([f"{first} via {group}", f"'{first}' via {group}", group])
    else:
        address = f"{local}@{rng.choice(PERSONAL_DOMAINS + WORK_DOMAINS)}"
    unsub = f"<mailto:{group}+unsubscribe@{host}>" if rng.random() < 0.8 else ""
    list_id = f"{group}.{host}"
    subject, preview = untidy(rng, subject, preview)
    return dict(lang=lang, name=name, address=address, subject=subject, preview=preview, unsub=unsub,
                list_id=list_id, list_post=rng.random() < 0.75, category="forums", kind=kind,
                target={"forums": 1.0})


def message(rng, half=None):
    category = rng.choices(list(SHARE), list(SHARE.values()))[0]
    if category == "primary":
        return primary(rng, half)
    if category == "forums":
        return forum(rng, half)
    return automated(rng, category, half)


def examples(rng, n, half=None):
    return [message(rng, half) for _ in range(n)]


def check():
    """Every kind has every language the other models have: an even and an
    odd alternative in those with several wordings, one in the others, all
    with only known placeholders; and each of the others has its products,
    topics, preheader, footer and currency."""
    import re
    known = {"{brand}", "{app}", "{name}", "{group}", "{topic}", "{item}", "{n}", "{pct}", "{price}",
             "{order}", "{day}", "{last4}", "{city}"}
    assert set(LANGUAGES) == set(WORDED) | set(OTHER_LANGUAGES), set(LANGUAGES) ^ (set(WORDED) | set(OTHER_LANGUAGES))
    kinds_all = {k for ks in KINDS.values() for k in ks}
    for lang, entries in OTHER_LANGUAGES.items():
        extra = {k for k in entries if not k.startswith("_")} - kinds_all
        assert not extra, (lang, extra)
        for key in ("_items", "_topics", "_preheader", "_footer", "_currency"):
            assert entries.get(key), (lang, key)
    for category, kinds in KINDS.items():
        for kind in kinds:
            for lang in LANGUAGES:
                alts = TEMPLATES[kind].get(lang)
                assert alts, (kind, lang)
                assert len(alts) >= (2 if lang in WORDED else 1), (kind, lang)
                for a in alts:
                    assert a.count(" || ") == 1, (kind, lang, a)
                    unknown = set(re.findall(r"\{\w+\}", a)) - known
                    assert not unknown, (kind, lang, unknown)


if __name__ == "__main__":
    check()
    rng = random.Random(1)
    for m in examples(rng, 12):
        print(f"--- {m['category']}/{m['kind']} ({m['lang']}) {m['target']}")
        print(f"From: {m['name']} <{m['address']}>  unsub={bool(m['unsub'])} list={m['list_id']} post={m['list_post']}")
        print(f"Subject: {m['subject']}\n{m['preview']}\n")
