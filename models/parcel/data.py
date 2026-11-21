"""Synthetic mail for the tracking number model.

Every number put in a message is one the app finds as a candidate: a
tracking_number_data test number (`numbers.json`) with a carrier's format
and check digit. What makes one a tracking number is only how the message
gives it, so the same numbers appear as tracking numbers in shipping mail
and as order, invoice, account, phone, booking and other numbers elsewhere,
and beside a tracking number in the same message. The model has to learn the
context, not the number.

Messages are put together from parts: a subject, a greeting, lines that
label a number ("Tracking number: {t}", "Bestellnummer {o}") or sentences
that mention one, filler and a footer. `{t}` is a tracking number, `{o}`
another number, `{c}` a carrier's name and `{n}` a name. Labels, sentences
and subjects are held out a quarter at a time for evaluation.
"""

import json
from pathlib import Path

NUMBERS = json.loads((Path(__file__).parent / "numbers.json").read_text(encoding="utf-8"))

CARRIERS = ["UPS", "FedEx", "DHL", "USPS", "DPD", "Canada Post", "OnTrac", "LaserShip",
            "Amazon Logistics", "Royal Mail", "La Poste", "Correos", "PostNL", "Poste Italiane",
            "GLS", "Hermes", "Japan Post", "ヤマト運輸", "顺丰速运", "CJ대한통운", "Почта России",
            "Yurtiçi Kargo", "InPost", "Correios", "PostNord", "Evri", "Purolator"]
NAMES = ["Sam", "Alex", "María", "Jonas", "Camille", "Giulia", "João", "Sven", "Ola", "Yuki",
         "Wei", "Minji", "Ivan", "Ayşe", "Priya", "Noah", "Emma", "Lukas", "Chloé", "Mateo"]

# Labels a tracking number is given under.
TRACK_LABELS = [
    "Tracking number", "Tracking #", "Tracking", "Tracking ID", "Tracking code", "Track your package",
    "Track your order", "Shipment tracking number", "Your tracking number is", "{c} tracking number",
    "{c} tracking", "Waybill", "Waybill number", "Air waybill", "AWB", "Consignment number",
    "Parcel number", "Shipment number", "Shipment ID", "Package ID", "Return tracking number",
    "Track & trace", "Delivery tracking", "Parcel ID", "Tracking reference",
    "Sendungsnummer", "Sendungsverfolgung", "Paketnummer", "Trackingnummer", "Sendungsnummer {c}",
    "Número de seguimiento", "Número de envío", "Código de seguimiento", "Localizador del envío",
    "Numéro de suivi", "Numéro de colis", "Suivi du colis", "Numéro d'envoi",
    "Numero di tracciamento", "Codice di tracking", "Numero di spedizione", "Codice spedizione",
    "Código de rastreio", "Código de rastreamento", "Número de rastreio", "Objeto",
    "Track & Trace code", "Zendingnummer", "Pakketnummer", "Barcode van je pakket",
    "Numer przesyłki", "Numer śledzenia", "Numer paczki",
    "Spårningsnummer", "Kolli-ID", "Sporingsnummer", "Seurantakoodi", "Forsendelsesnummer",
    "追跡番号", "お問い合わせ伝票番号", "伝票番号", "送り状番号", "配送伝票番号", "荷物番号",
    "运单号", "快递单号", "物流单号", "追踪号码", "包裹单号", "物流追蹤號碼",
    "운송장 번호", "송장번호", "배송 추적 번호", "운송장번호",
    "Трек-номер", "Номер отслеживания", "Номер отправления", "Трек номер посылки",
    "Трек-номер відправлення", "Номер посилки",
    "Kargo takip numarası", "Takip numarası", "Gönderi numarası", "Kargo takip no",
    "رقم التتبع", "رقم الشحنة", "मार्ग संख्या", "ट्रैकिंग नंबर", "Nomor resi", "Mã vận đơn",
    "Αριθμός αποστολής", "Číslo zásilky", "Sledovací číslo",
]

# Labels another number of the same kinds is given under.
OTHER_LABELS = [
    "Order number", "Order #", "Order", "Order ID", "Invoice", "Invoice number", "Invoice #",
    "Customer number", "Customer ID", "Account number", "Account", "Account ending",
    "Reference", "Reference number", "Booking reference", "Confirmation number", "Ticket number",
    "Transaction ID", "Payment ID", "Receipt number", "Case number", "Ticket", "Serial number",
    "Membership ID", "Member number", "Policy number", "Claim number", "Gift card number",
    "Meeting ID", "Call us at", "Phone", "Tel.", "Fax", "Support line", "Item number", "SKU",
    "Product code", "Model number", "IMEI", "Voucher code", "Loyalty number", "Card number",
    "Bestellnummer", "Rechnungsnummer", "Kundennummer", "Auftragsnummer", "Vertragsnummer",
    "Telefon", "Artikelnummer", "Belegnummer",
    "Número de pedido", "Factura", "Número de cliente", "Referencia", "Teléfono", "Número de factura",
    "Numéro de commande", "Facture", "Numéro client", "Référence", "Téléphone", "N° de facture",
    "Numero d'ordine", "Fattura", "Codice cliente", "Telefono",
    "Número do pedido", "Nota fiscal", "Número do cliente", "Telefone",
    "Ordernummer", "Factuurnummer", "Klantnummer", "Bestelnummer",
    "Numer zamówienia", "Faktura nr", "Numer klienta", "Telefon",
    "Ordernummer", "Kundnummer", "Fakturanummer", "Bestillingsnummer", "Tilausnumero",
    "注文番号", "ご注文番号", "請求書番号", "お客様番号", "会員番号", "電話番号", "受付番号",
    "订单号", "订单编号", "发票号码", "客户编号", "会员号", "电话", "訂單編號",
    "주문 번호", "주문번호", "청구서 번호", "고객 번호", "회원번호", "전화",
    "Номер заказа", "Счёт №", "Номер клиента", "Телефон", "Номер договора", "Номер замовлення",
    "Sipariş numarası", "Fatura numarası", "Müşteri numarası", "Telefon",
    "رقم الطلب", "رقم الفاتورة", "ऑर्डर नंबर", "Nomor pesanan", "Mã đơn hàng",
    "Αριθμός παραγγελίας", "Číslo objednávky", "Číslo faktury",
]

# How a label and its number are written.
LABEL_FORMS = ["{L}: {x}", "{L}: {x}", "{L} {x}", "{L}:\n{x}", "{L}\n{x}", "{L} #{x}", "{L}：{x}",
               "{L} - {x}", "{L}: {x}.", "{L} ({x})"]

# Sentences that give a tracking number.
TRACK_SENTENCES = [
    "Your package is on its way. Track it with {t}.",
    "Your parcel {t} is out for delivery today.",
    "Shipment {t} is delayed due to weather.",
    "Use {t} to follow your parcel on the {c} website.",
    "We handed your parcel to {c}; it can be tracked with {t}.",
    "Your package with tracking number {t} was delivered.",
    "{c} picked up shipment {t} this morning.",
    "Track shipment {t} at any time on our website.",
    "Your return {t} has been received by {c}.",
    "Ihre Sendung {t} wird heute zugestellt.",
    "Ihr Paket {t} ist unterwegs.",
    "Tu envío {t} está en reparto.",
    "Puedes seguir tu paquete con el número {t}.",
    "Votre colis {t} sera livré aujourd'hui.",
    "Suivez votre colis {t} sur le site de {c}.",
    "Il tuo pacco {t} è in consegna.",
    "Sua encomenda {t} saiu para entrega.",
    "Je pakket {t} wordt vandaag bezorgd.",
    "Twoja paczka {t} jest w drodze.",
    "Ditt paket {t} levereras i dag.",
    "お荷物（{t}）は本日配達予定です。",
    "您的包裹{t}正在派送中，请保持电话畅通。",
    "고객님의 택배({t})가 배송 중입니다.",
    "Ваша посылка {t} передана в доставку.",
    "{t} numaralı kargonuz dağıtıma çıktı.",
]

# Sentences that give another number.
OTHER_SENTENCES = [
    "Thanks for your order {o}. We'll email you a tracking number when it ships.",
    "Your payment for invoice {o} was received.",
    "Please quote your customer number {o} when you contact us.",
    "Questions? Call us at {o}, Monday to Friday.",
    "Your booking {o} is confirmed. Check-in from 3 pm.",
    "We issued a refund for order {o}. It may take 5-10 days to appear.",
    "Your card ending in {o} was charged $45.10.",
    "Order {o} is being prepared and will ship soon.",
    "Your warranty for serial number {o} runs until 2028.",
    "Your support case {o} has been closed.",
    "Ihre Bestellung {o} ist bei uns eingegangen.",
    "Die Rechnung {o} ist beigefügt.",
    "Hemos recibido tu pedido {o}.",
    "La factura {o} está disponible en tu cuenta.",
    "Nous avons bien reçu votre commande {o}.",
    "Abbiamo ricevuto il tuo ordine {o}.",
    "Recebemos o seu pedido {o}.",
    "We hebben je bestelling {o} ontvangen.",
    "Otrzymaliśmy Twoje zamówienie {o}.",
    "Vi har tagit emot din beställning {o}.",
    "ご注文（{o}）を承りました。",
    "我们已收到您的订单{o}。",
    "주문({o})이 접수되었습니다.",
    "Мы получили ваш заказ {o}.",
    "{o} numaralı siparişiniz alındı.",
]

SHIP_SUBJECTS = [
    "Your order has shipped", "Your package is on the way", "Shipping confirmation",
    "Out for delivery", "Delivered: your package", "Delivery update", "Your parcel is coming",
    "Shipment notification", "Your return label", "Your {c} delivery", "Order dispatched",
    "Ihre Bestellung wurde versandt", "Ihr Paket kommt heute", "Versandbestätigung",
    "Tu pedido ha sido enviado", "Confirmación de envío", "Votre commande a été expédiée",
    "Votre colis arrive", "Il tuo ordine è stato spedito", "Seu pedido foi enviado",
    "Je bestelling is verzonden", "Twoje zamówienie zostało wysłane", "Din order har skickats",
    "ご注文の商品を発送しました", "発送のお知らせ", "您的订单已发货", "包裹派送中",
    "주문하신 상품이 발송되었습니다", "Ваш заказ отправлен", "Siparişiniz kargoya verildi",
]
OTHER_SUBJECTS = [
    "Order confirmation", "Your invoice", "Payment received", "Your account", "Booking confirmed",
    "Your ticket", "Membership renewal", "Support request received", "Your warranty",
    "Your statement is ready", "Refund issued", "Your receipt", "Meeting details", "Your policy",
    "Thanks for your order", "Welcome!", "Bestellbestätigung", "Ihre Rechnung", "Kundenkonto",
    "Confirmación de pedido", "Tu factura", "Confirmation de commande", "Votre facture",
    "Conferma d'ordine", "Confirmação de pedido", "Orderbevestiging", "Potwierdzenie zamówienia",
    "Orderbekräftelse", "ご注文の確認", "订单确认", "주문 확인", "Подтверждение заказа",
    "Sipariş onayı", "",
]

GREETINGS = ["", "", "Hi {n},\n\n", "Hello {n},\n\n", "Dear {n},\n\n", "Hallo {n},\n\n",
             "Hola {n},\n\n", "Bonjour {n},\n\n", "{n}様\n\n", "{n}您好，\n\n", "Здравствуйте, {n}!\n\n"]
SHIP_FILLER = [
    "Good news! Your order is on its way.", "Your items have been dispatched.",
    "Estimated delivery: 3-5 business days.", "Carrier: {c}", "Shipped via {c}",
    "Delivery scheduled for Friday.", "Left at: front door", "Ship to: {n}",
    "Ihr Paket ist unterwegs.", "Tu pedido está en camino.", "Votre colis est en route.",
    "ご注文の商品を発送いたしました。", "您的订单已发货。", "상품이 발송되었습니다.", "Ваш заказ отправлен.",
]
FILLER = [
    "Thanks for shopping with us.", "Total: $45.10", "Subtotal: $39.90", "Qty: 2",
    "Need help? Visit our help center.", "This is an automated message, please don't reply.",
    "Vielen Dank für Ihren Einkauf.", "Gracias por tu compra.", "Merci de votre confiance.",
    "ご利用ありがとうございます。", "感谢您的惠顾。", "감사합니다.", "Спасибо за покупку!",
]
FOOTERS = [
    "", "\n\n© 2026 Example Store, 500 Main St, Springfield, IL 62701",
    "\n\nUnsubscribe | Privacy", "\n\nExample GmbH, Hauptstraße 5, 10115 Berlin",
]


def split(items):
    """Training and held-out items: every fourth is held out."""
    return ([t for i, t in enumerate(items) if i % 4 != 3],
            [t for i, t in enumerate(items) if i % 4 == 3])


def parts(held_out=None):
    """The pools messages are built from: the training share, the held-out
    share (`held_out=True`), or everything (None)."""
    pools = dict(track_labels=TRACK_LABELS, other_labels=OTHER_LABELS,
                 track_sentences=TRACK_SENTENCES, other_sentences=OTHER_SENTENCES,
                 ship_subjects=SHIP_SUBJECTS, other_subjects=OTHER_SUBJECTS)
    if held_out is None:
        return pools
    return {k: split(v)[1 if held_out else 0] for k, v in pools.items()}


def written(rng, number):
    """A number as mail writes it: whole, or long digit runs in groups of
    four (no group shorter than two, as the app joins them)."""
    if number.isdigit() and len(number) >= 16 and rng.random() < 0.3:
        groups = [number[i:i + 4] for i in range(0, len(number), 4)]
        if len(groups[-1]) < 2:
            groups[-2:] = [groups[-2][:2], groups[-2][2:] + groups[-1]]
        return " ".join(groups)
    return number


def fill(rng, template):
    """The template with its slots filled, and (start, end, label) of each
    number in it."""
    out, spans = [], []
    at = 0
    i = 0
    while i < len(template):
        if template[i] == "{" and template[i + 2:i + 3] == "}":
            slot = template[i + 1]
            if slot in "to":
                value = written(rng, rng.choice(NUMBERS)["number"])
                spans.append((at, at + len(value), 1 if slot == "t" else 0))
            elif slot == "c":
                value = rng.choice(CARRIERS)
            else:
                value = rng.choice(NAMES)
            out.append(value)
            at += len(value)
            i += 3
        else:
            out.append(template[i])
            at += 1
            i += 1
    return "".join(out), spans


def number_line(rng, pools, kind):
    """A line giving a number of `kind` ("t" or "o"): a label or a sentence."""
    if rng.random() < 0.65:
        label = rng.choice(pools["track_labels" if kind == "t" else "other_labels"])
        return rng.choice(LABEL_FORMS).replace("{L}", label).replace("{x}", "{" + kind + "}")
    return rng.choice(pools["track_sentences" if kind == "t" else "other_sentences"])


def message(rng, pools):
    """(subject, body, [(start, end, label)]) with places in the text the
    model reads: the subject, a newline and the body. Shipping mail gives a
    tracking number, often beside other numbers; other mail gives only other
    numbers (sometimes under a shipping subject, before the order ships)."""
    shipping = rng.random() < 0.5
    if shipping:
        subject = rng.choice(pools["ship_subjects"])
        lines = [number_line(rng, pools, "t")]
        lines += [number_line(rng, pools, "o") for _ in range(rng.choice([0, 0, 1, 1, 2]))]
        lines += [rng.choice(SHIP_FILLER) for _ in range(rng.choice([0, 1, 2]))]
    else:
        subject = rng.choice(pools["other_subjects"] if rng.random() < 0.85 else pools["ship_subjects"])
        lines = [number_line(rng, pools, "o") for _ in range(rng.choice([1, 1, 2, 3]))]
    lines += [rng.choice(FILLER) for _ in range(rng.choice([0, 1, 2]))]
    rng.shuffle(lines)
    body_t = rng.choice(GREETINGS) + "\n".join(lines) + rng.choice(FOOTERS)
    subject, sspans = fill(rng, subject)
    body, bspans = fill(rng, body_t)
    offset = len(subject) + 1
    spans = sspans + [(s + offset, e + offset, y) for s, e, y in bspans]
    return subject, body, spans


def messages(rng, pools, n):
    return [message(rng, pools) for _ in range(n)]
