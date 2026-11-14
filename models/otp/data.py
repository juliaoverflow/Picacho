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
    ("Your {svc} one-time code", "Hello,\n\n{code} is the one-time {svc} code that was requested for an account that belongs to you or your child. To log in, enter it on the login screen. This code will expire in {min} minutes."),
    ("{svc} account", "Hi {name},\n\nhere is the one-time code that was just requested for the account you or someone in your family uses on {svc}: {code}. Enter it on the screen where you are signing in."),
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

# Sign-in messages in more languages. A third element names the digits the
# code may be written in besides ASCII ("arab", "persian", "fullwidth",
# "deva", "thai", "beng").
POSITIVE_MORE = [
    # Japanese
    ("{svc} 認証コード", "{name}様\n\n{svc}の認証コードは {code} です。\nこのコードの有効期限は{min}分です。心当たりがない場合は、このメールを破棄してください。", "fullwidth"),
    ("ログイン確認コードのお知らせ", "ログインするには、次の確認コードを入力してください：{code}\n他の人には教えないでください。", "fullwidth"),
    ("【{svc}】ワンタイムパスワード", "ワンタイムパスワード：{code}\n有効期限：{min}分", "fullwidth"),
    # Chinese (Simplified)
    ("{svc}验证码", "您的验证码是{code}，{min}分钟内有效。请勿将验证码告诉他人。", "fullwidth"),
    ("登录验证", "您正在登录{svc}，验证码：{code}。如非本人操作，请忽略本邮件。", "fullwidth"),
    ("安全验证码", "【{svc}】动态密码{code}，用于身份验证，有效期{min}分钟。", ""),
    # Chinese (Traditional)
    ("{svc} 驗證碼", "您的驗證碼為 {code}，請於{min}分鐘內輸入。請勿與他人分享此驗證碼。", "fullwidth"),
    ("登入驗證碼", "您好，您的登入驗證碼是：{code}\n若非本人操作，請立即變更密碼。", ""),
    # Korean
    ("{svc} 인증번호", "{svc} 인증번호는 [{code}] 입니다. {min}분 이내에 입력해 주세요.", ""),
    ("로그인 인증 코드", "로그인을 완료하려면 다음 인증 코드를 입력하세요: {code}\n본인이 요청하지 않았다면 이 메일을 무시하세요.", ""),
    # Russian
    ("Код подтверждения {svc}", "Ваш код подтверждения: {code}\nКод действителен {min} минут. Никому не сообщайте этот код.", ""),
    ("Вход в аккаунт", "Здравствуйте, {name}!\nДля входа в {svc} введите код {code}. Если это были не вы, смените пароль.", ""),
    ("Одноразовый пароль", "Одноразовый пароль для входа: {code}", ""),
    # Ukrainian
    ("Код підтвердження", "Ваш код підтвердження {svc}: {code}. Код дійсний протягом {min} хвилин.", ""),
    ("Вхід до облікового запису", "Щоб увійти, введіть цей код: {code}\nНікому не повідомляйте його.", ""),
    # Turkish
    ("{svc} doğrulama kodu", "{svc} doğrulama kodunuz: {code}\nBu kod {min} dakika geçerlidir. Kodu kimseyle paylaşmayın.", ""),
    ("Giriş kodunuz", "Hesabınıza giriş yapmak için tek kullanımlık şifreniz {code}.", ""),
    # Arabic
    ("رمز التحقق من {svc}", "رمز التحقق الخاص بك هو {code}\nصالح لمدة {min} دقائق. لا تشارك هذا الرمز مع أي شخص.", "arab"),
    ("تسجيل الدخول", "مرحباً {name}، استخدم الرمز {code} لتسجيل الدخول إلى حسابك.", "arab"),
    ("كلمة المرور لمرة واحدة", "كلمة المرور لمرة واحدة: {code}\nإذا لم تطلب هذا الرمز، يرجى تجاهل هذه الرسالة.", "arab"),
    # Persian
    ("کد تأیید {svc}", "کد تأیید شما: {code}\nاین کد تا {min} دقیقه معتبر است. آن را در اختیار دیگران قرار ندهید.", "persian"),
    ("ورود به حساب", "برای ورود به حساب کاربری خود، رمز یکبار مصرف {code} را وارد کنید.", "persian"),
    # Hebrew
    ("קוד האימות שלך", "קוד האימות שלך ל-{svc} הוא {code}. הקוד בתוקף ל-{min} דקות.", ""),
    ("כניסה לחשבון", "להשלמת הכניסה הזן את הקוד הבא: {code}\nאל תשתף קוד זה עם אף אחד.", ""),
    # Hindi
    ("{svc} सत्यापन कोड", "आपका सत्यापन कोड {code} है। यह {min} मिनट के लिए मान्य है। इसे किसी के साथ साझा न करें।", "deva"),
    ("लॉगिन ओटीपी", "{svc} में लॉगिन करने के लिए आपका OTP {code} है।", "deva"),
    # Bengali
    ("যাচাইকরণ কোড", "আপনার {svc} যাচাইকরণ কোড হলো {code}। এটি {min} মিনিটের জন্য বৈধ।", "beng"),
    # Indonesian
    ("Kode verifikasi {svc}", "Kode verifikasi Anda adalah {code}. Kode berlaku selama {min} menit. Jangan berikan kode ini kepada siapa pun.", ""),
    ("Kode masuk Anda", "Gunakan kode OTP {code} untuk masuk ke akun {svc} Anda.", ""),
    # Malay
    ("Kod pengesahan", "Kod pengesahan {svc} anda ialah {code}. Sah selama {min} minit.", ""),
    # Vietnamese
    ("Mã xác minh {svc}", "Mã xác minh của bạn là {code}. Mã có hiệu lực trong {min} phút. Không chia sẻ mã này với bất kỳ ai.", ""),
    ("Mã đăng nhập", "Nhập mã OTP {code} để đăng nhập vào tài khoản {svc}.", ""),
    # Thai
    ("รหัสยืนยัน {svc}", "รหัสยืนยันของคุณคือ {code} รหัสนี้ใช้ได้ภายใน {min} นาที ห้ามบอกรหัสนี้กับผู้อื่น", "thai"),
    ("รหัส OTP สำหรับเข้าสู่ระบบ", "รหัส OTP ของคุณคือ {code} (อ้างอิง: {promo})", "thai"),
    # Greek
    ("Κωδικός επαλήθευσης", "Ο κωδικός επαλήθευσης για το {svc} είναι {code}. Ισχύει για {min} λεπτά.", ""),
    ("Σύνδεση στον λογαριασμό", "Για να συνδεθείτε, εισαγάγετε τον κωδικό μίας χρήσης: {code}", ""),
    # Czech
    ("Ověřovací kód", "Váš ověřovací kód pro {svc} je {code}. Platí {min} minut.", ""),
    ("Přihlašovací kód", "Pro přihlášení zadejte tento kód: {code}\nKód nikomu nesdělujte.", ""),
    # Slovak
    ("Overovací kód", "Váš overovací kód je {code}. Kód je platný {min} minút.", ""),
    # Danish
    ("Din bekræftelseskode", "Din {svc} bekræftelseskode er {code}. Koden udløber om {min} minutter.", ""),
    # Norwegian
    ("Engangskode", "Din engangskode for innlogging er {code}. Ikke del koden med andre.", ""),
    # Finnish
    ("Vahvistuskoodi", "{svc}-vahvistuskoodisi on {code}. Koodi on voimassa {min} minuuttia.", ""),
    ("Kirjautumiskoodi", "Kirjaudu sisään syöttämällä koodi {code}.", ""),
    # Romanian
    ("Cod de verificare", "Codul tău de verificare {svc} este {code}. Este valabil {min} minute.", ""),
    ("Codul de autentificare", "Folosește codul {code} pentru a te autentifica. Nu împărtăși acest cod.", ""),
    # Hungarian
    ("Ellenőrző kód", "A(z) {svc} ellenőrző kódod: {code}. A kód {min} percig érvényes.", ""),
    ("Bejelentkezési kód", "A bejelentkezéshez add meg ezt a kódot: {code}", ""),
    # Catalan
    ("Codi de verificació", "El teu codi de verificació de {svc} és {code}. Caduca en {min} minuts.", ""),
    # Croatian / Serbian (Latin)
    ("Kod za potvrdu", "Vaš kod za potvrdu je {code}. Kod vrijedi {min} minuta.", ""),
    # Bulgarian
    ("Код за потвърждение", "Вашият код за потвърждение е {code}. Валиден е {min} минути.", ""),
    # Tagalog
    ("Iyong verification code", "Ang iyong {svc} verification code ay {code}. Huwag itong ibahagi kahit kanino.", ""),
    # Swahili
    ("Msimbo wa uthibitisho", "Msimbo wako wa uthibitisho wa {svc} ni {code}. Usimpe mtu yeyote.", ""),
    # Second wordings
    ("আপনার লগইন কোড", "লগইন করতে এই কোডটি ব্যবহার করুন: {code}। কোডটি কারো সাথে শেয়ার করবেন না।", "beng"),
    ("Kod log masuk", "Gunakan kod {code} untuk log masuk ke akaun {svc} anda.", ""),
    ("Prihlasovací kód", "Na prihlásenie zadajte kód {code}. Kód nikomu neposielajte.", ""),
    ("Log ind på {svc}", "Brug koden {code} til at logge ind. Del ikke koden med andre.", ""),
    ("Bekreftelseskode", "Din bekreftelseskode for {svc} er {code}. Koden er gyldig i {min} minutter.", ""),
    ("Codi d'inici de sessió", "Introdueix el codi {code} per iniciar la sessió a {svc}.", ""),
    ("Kod za prijavu", "Za prijavu unesite kod {code}. Ne dijelite ga ni s kim.", ""),
    ("Код за вход", "Използвайте код {code}, за да влезете в профила си в {svc}.", ""),
    ("Code para mag-login", "Gamitin ang code na {code} para mag-login sa {svc}. Mag-e-expire ito sa loob ng {min} minuto.", ""),
    ("Msimbo wa kuingia", "Tumia msimbo {code} kuingia kwenye akaunti yako ya {svc}.", ""),
    ("{svc} 로그인 코드", "인증번호 {code}를 입력하여 로그인을 완료하세요.", ""),
    ("ログインコード", "{svc}のログインコード：{code}\nこのコードは{min}分間有効です。", "fullwidth"),
    ("验证码通知", "【{svc}】{code}（登录验证码），请勿泄露。", ""),
    ("رمز الدخول", "رمز الدخول إلى {svc}: {code}\nلا تشارك هذا الرمز.", "arab"),
    ("کد ورود", "کد ورود شما به {svc}: {code}", "persian"),
    ("קוד חד פעמי", "הקוד החד פעמי שלך: {code}", ""),
    ("ओटीपी कोड", "{svc} के लिए आपका वन टाइम पासवर्ड {code} है। यह {min} मिनट में समाप्त हो जाएगा।", "deva"),
    ("Mã OTP của bạn", "Mã OTP {svc} của bạn là {code}, hết hạn sau {min} phút.", ""),
    ("รหัสเข้าสู่ระบบ", "ใช้รหัส {code} เพื่อเข้าสู่ระบบ {svc}", "thai"),
    ("Κωδικός ασφαλείας {svc}", "Ο κωδικός ασφαλείας σας: {code}. Μην τον κοινοποιήσετε σε κανέναν.", ""),
    ("Kod jednorázový", "Váš jednorázový kód je {code}.", ""),
    ("Kertakäyttöinen koodi", "Kertakäyttöinen koodisi: {code}. Älä jaa koodia kenellekään.", ""),
    ("Egyszer használatos kód", "Az egyszer használatos kódod: {code}", ""),
    ("Cod unic", "Codul tău unic de acces este {code}.", ""),
    ("Tek kullanımlık kod", "{svc} hesabınız için doğrulama kodu {code}. {min} dakika içinde kullanın.", ""),
    ("Код для входу", "Ваш одноразовий код для входу до {svc}: {code}", ""),
    ("Код безопасности", "Код безопасности {svc}: {code}. Не сообщайте его никому, даже сотрудникам поддержки.", ""),
    ("Kode OTP", "JANGAN BERIKAN kode ini kepada siapa pun. Kode OTP {svc}: {code}", ""),
    # More in the languages already covered
    ("Ihr Einmalpasswort", "Ihr Einmalpasswort (OTP) lautet: {code}\nEs verliert nach {min} Minuten seine Gültigkeit.", ""),
    ("Votre mot de passe à usage unique", "Votre code à usage unique est le {code}. Ne le partagez avec personne.", ""),
    ("Tu código de un solo uso", "Tu contraseña de un solo uso es {code}. No la compartas con nadie.", ""),
    ("Seu código de login", "Use o código {code} para entrar na sua conta {svc}. Válido por {min} minutos.", ""),
    ("Codice di sicurezza", "Inserisci il codice {code} per completare l'accesso a {svc}.", ""),
    ("Eenmalige code", "Je eenmalige inlogcode is {code}. Deel deze code met niemand.", ""),
    ("Kod do logowania", "Twój jednorazowy kod do logowania: {code}", ""),
    ("Inloggningskod", "Ange koden {code} för att logga in på {svc}.", ""),
    # The code far from the words that say what it is: at the start of a
    # long sentence that names it only at the end, or at the end of one that
    # named it at the start (as Roblox writes in every language).
    ("{svc}のワンタイムコード", "こんにちは。\n\n{code}は、ご自身またはお子様のアカウントに対してリクエストされた{svc}のワンタイムコードです。ログインするには、このコードをログイン画面に入力してください。このコードは{min}分で期限切れになります。", "fullwidth"),
    ("{svc}からのお知らせ", "{name}様\n\n{code}が、先ほどお客様のアカウントでのサインインのためにリクエストされた確認用のコードです。画面に入力して手続きを完了してください。", "fullwidth"),
    ("{svc}-Code", "Hallo,\n\ndies ist ein einmaliger {svc}-Code, der für ein Konto angefordert wurde, das dir oder deinem Kind gehört: {code}. Gib ihn im Anmeldebildschirm ein, um dich anzumelden. Der Code ist {min} Minuten lang gültig.", ""),
    ("Ihr Anmeldecode", "Guten Tag {name},\n\nSie haben soeben über die App oder die Website eine Anmeldung bei Ihrem Konto angefordert, hier ist der Bestätigungscode dafür: {code}. Er läuft in {min} Minuten ab.", ""),
    ("{svc}", "{code} ist der einmalige Code, den du vor kurzem für dein Konto bei {svc} angefordert hast. Gib ihn ein, um fortzufahren.", ""),
    ("Votre code {svc}", "Bonjour,\n\nvoici un code {svc} à usage unique demandé pour un compte qui vous appartient ou qui appartient à votre enfant : {code}. Saisissez-le sur l'écran de connexion pour vous connecter. Ce code expire dans {min} minutes.", ""),
    ("Tu código de {svc}", "Hola:\n\n{code} es el código de un solo uso de {svc} solicitado para una cuenta tuya o de tu hijo. Introdúcelo en la pantalla de inicio de sesión para iniciar sesión. Este código caduca en {min} minutos.", ""),
    ("Seu código {svc}", "Olá,\n\neste é um código único do {svc} solicitado para uma conta que pertence a você ou ao seu filho: {code}. Insira-o na tela de login para entrar. O código expira em {min} minutos.", ""),
    ("Il tuo codice {svc}", "Ciao,\n\n{code} è il codice monouso di {svc} richiesto per un account che appartiene a te o a tuo figlio. Inseriscilo nella schermata di accesso per accedere. Il codice scade tra {min} minuti.", ""),
    ("Je {svc}-code", "Hallo,\n\ndit is een eenmalige {svc}-code die is aangevraagd voor een account van jou of je kind: {code}. Voer hem in op het inlogscherm om in te loggen. De code verloopt over {min} minuten.", ""),
    ("Twój kod {svc}", "Cześć,\n\n{code} to jednorazowy kod {svc}, o który poproszono dla konta należącego do Ciebie lub Twojego dziecka. Wpisz go na ekranie logowania, aby się zalogować. Kod wygaśnie za {min} minut.", ""),
    ("Ваш код {svc}", "Здравствуйте!\n\nЭто одноразовый код {svc}, запрошенный для учётной записи, принадлежащей вам или вашему ребёнку: {code}. Введите его на экране входа. Код действителен {min} минут.", ""),
    ("{svc} 일회용 코드", "안녕하세요.\n\n{code}은(는) 회원님 또는 자녀의 계정에 대해 요청된 {svc} 일회용 코드입니다. 로그인하려면 로그인 화면에 이 코드를 입력하세요. 이 코드는 {min}분 후에 만료됩니다.", ""),
    ("{svc} 一次性代码", "您好！\n\n{code} 是为您或您孩子的账户申请的 {svc} 一次性代码。请在登录界面输入此代码进行登录。此代码将在 {min} 分钟后过期。", "fullwidth"),
    ("{svc} 一次性代碼", "您好：\n\n這是為您或您孩子的帳戶所要求的 {svc} 一次性代碼：{code}。請在登入畫面輸入此代碼。此代碼將於 {min} 分鐘後失效。", ""),
    ("{svc} kodunuz", "Merhaba,\n\n{code}, size veya çocuğunuza ait bir hesap için istenen tek kullanımlık {svc} kodudur. Giriş yapmak için bu kodu giriş ekranına girin. Kod {min} dakika içinde sona erer.", ""),
    ("Din {svc}-kod", "Hej,\n\nhär är en engångskod från {svc} som har begärts för ett konto som tillhör dig eller ditt barn: {code}. Ange den på inloggningsskärmen för att logga in. Koden upphör att gälla om {min} minuter.", ""),
]

# Mail with numbers that aren't codes, in more languages.
NEGATIVE_MORE = [
    ("ご注文の確認（注文番号 {num8}）", "{name}様\nご注文ありがとうございます。注文番号：{num8}\nお届け予定日：{date}\n合計金額：{amount}\nお問い合わせ：{phone}"),
    ("発送のお知らせ", "ご注文の商品を発送しました。追跡番号：{num10}\n配送業者のサイトで状況をご確認いただけます。"),
    ("您的订单已发货", "您的订单{num8}已发货，运单号：{num10}。预计{date}送达。订单金额：{amount}。"),
    ("发票通知", "发票号码：{num8}\n开票日期：{date}\n金额：{amount}\n客服电话：{phone}"),
    ("訂單確認", "感謝您的訂購！訂單編號：{num8}，總金額：{amount}，預計出貨日期：{date}。"),
    ("주문 확인", "주문해 주셔서 감사합니다. 주문번호: {num8}\n결제 금액: {amount}\n배송 예정일: {date}"),
    ("Ваш заказ {num7} отправлен", "Ваш заказ №{num7} отправлен. Трек-номер: {ref}. Сумма: {amount}. Телефон поддержки: {phone}."),
    ("Счёт на оплату", "Счёт №{num6} от {date}. Сумма к оплате: {amount}. Лицевой счёт: {num10}."),
    ("Замовлення {num6}", "Ваше замовлення №{num6} доставлено. Номер відправлення: {num10}."),
    ("Siparişiniz kargoya verildi", "{num8} numaralı siparişiniz kargoya verildi. Takip numarası: {num10}. Toplam: {amount}."),
    ("تأكيد الطلب", "شكراً لطلبك. رقم الطلب: {num8}\nالمبلغ الإجمالي: {amount}\nتاريخ التسليم المتوقع: {date}"),
    ("فاتورة رقم {num6}", "فاتورتك رقم {num6} بتاريخ {date} بمبلغ {amount}. رقم الحساب: {num10}."),
    ("سفارش شما ارسال شد", "سفارش شماره {num7} ارسال شد. کد رهگیری: {num10}."),
    ("ההזמנה שלך נשלחה", "הזמנה מספר {num8} נשלחה. מספר מעקב: {ref}. סה\"כ: {amount}."),
    ("आपका ऑर्डर भेज दिया गया है", "आपका ऑर्डर {num8} भेज दिया गया है। ट्रैकिंग नंबर: {num10}। कुल राशि: {amount}।"),
    ("Pesanan Anda telah dikirim", "Pesanan {num8} telah dikirim. Nomor resi: {ref}. Total: {amount}. Hubungi kami di {phone}."),
    ("Đơn hàng {num7} đã được giao", "Đơn hàng số {num7} đã được giao. Mã vận đơn: {num10}. Tổng cộng: {amount}."),
    ("คำสั่งซื้อของคุณถูกจัดส่งแล้ว", "หมายเลขคำสั่งซื้อ {num8} หมายเลขพัสดุ {ref} ยอดรวม {amount}"),
    ("Η παραγγελία σας", "Η παραγγελία {num7} απεστάλη. Αριθμός αποστολής: {num10}. Σύνολο: {amount}."),
    ("Vaše objednávka {num7}", "Objednávka č. {num7} byla odeslána. Číslo zásilky: {num10}. Celkem: {amount}."),
    ("Din ordre {num6}", "Tak for din ordre {num6}. Forventet levering: {date}. Ordrenummer {num6}."),
    ("Tilauksesi {num7}", "Tilauksesi {num7} on lähetetty. Seurantanumero: {ref}."),
    ("Comanda ta {num7}", "Comanda nr. {num7} a fost expediată. AWB: {num10}. Total: {amount}."),
    ("Számla {num7}", "Számlaszám: {num7}\nÖsszeg: {amount}\nFizetési határidő: {date}"),
    ("Kupon kode", "Gunakan kode promo {promo} untuk diskon {pct}% sampai {date}."),
    ("优惠券", "使用优惠码{promo}即可享受{pct}%折扣，有效期至{date}。"),
    ("クーポンのご案内", "クーポンコード「{promo}」で{pct}%オフ。{date}まで有効です。"),
    ("Промокод", "Используйте промокод {promo} и получите скидку {pct}% до {date}."),
    ("Ordrebekræftelse", "Ordrenummer: {num7}\nBeløb: {amount}\nLevering: {date}"),
    ("Tilausvahvistus", "Tilausnumero {num8}. Summa {amount}. Toimitus {date}."),
    ("Faktura {num6}", "Faktura nr {num6}. Kwota do zapłaty: {amount}. Termin: {date}."),
    ("Bestillingsbekreftelse", "Takk for bestillingen. Ordrenummer {num7}, totalt {amount}."),
    ("Potvrda narudžbe", "Broj narudžbe: {num7}. Ukupno: {amount}. Dostava: {date}."),
    ("Поръчка {num6}", "Вашата поръчка №{num6} е изпратена. Номер на пратката: {num10}."),
    ("Rendelés visszaigazolása", "Rendelésszám: {num8}. Végösszeg: {amount}."),
    ("Comanda confirmată", "Comanda {num7} a fost confirmată. Total de plată: {amount}."),
    ("Confirmació de la comanda", "Número de comanda: {num7}. Import: {amount}."),
    ("Kumpirmasyon ng order", "Ang iyong order {num7} ay naipadala na. Tracking number: {ref}."),
    ("Agizo lako", "Agizo lako namba {num7} limetumwa. Jumla: {amount}."),
    ("订单号 {num8}", "订单号：{num8}\n客服电话：{phone}\n地址：{street}"),
    ("会員番号のお知らせ", "会員番号：{num8}\nお客様番号：{num7}\nお問い合わせ：{phone}"),
    ("예약 확인", "예약번호: {num8}\n체크인: {date}\n고객센터: {phone}"),
    ("Номер договора", "Номер договора: {num8}. Лицевой счёт: {num10}. Телефон: {phone}."),
    ("Rezervasyon onayı", "Rezervasyon numaranız: {ref}. Giriş tarihi: {date}."),
    ("رقم الحجز", "رقم الحجز: {ref}\nرقم العميل: {num7}"),
    ("Mã đặt chỗ", "Mã đặt chỗ của bạn: {ref}. Ngày nhận phòng: {date}."),
    ("หมายเลขการจอง", "หมายเลขการจองของคุณคือ {ref} วันที่เข้าพัก {date}"),
    ("Nomor pelanggan", "Nomor pelanggan: {num8}. Tagihan bulan ini: {amount}."),
    ("बुकिंग की पुष्टि", "आपकी बुकिंग संख्या {ref} है। यात्रा की तारीख {date}।"),
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


# Other numbers that turn up beside a code (an order, a size, a customer
# number), in many languages: they stay unlabelled in a sign-in message.
DISTRACTORS = [
    "Order #{num6} is unaffected.", "Your order {num7} has shipped.", "Customer ID: {num7}.",
    "Ticket {num6} remains open.", "Reference: {ref}.", "Account ending in {num4}.",
    "Pedido {num5} enviado.", "Número de cliente: {num7}.", "Bestellung {num7} versendet.",
    "Größe {num4}.", "Kundennummer: {num6}.", "Commande n° {num6} expédiée.", "Ordine {num6} spedito.",
    "Bestelling {num7} verzonden.", "Zamówienie {num6} wysłane.", "Заказ №{num6} отправлен.",
    "Номер договора {num8}.", "Замовлення {num6} доставлено.", "Sipariş {num7} kargoda.",
    "订单号：{num8}。", "会員番号：{num8}", "주문번호: {num8}", "رقم الطلب: {num7}",
    "سفارش {num6}", "הזמנה {num7}", "ऑर्डर {num7}", "Pesanan {num7} dikirim.",
    "Đơn hàng {num7}.", "คำสั่งซื้อ {num7}", "Παραγγελία {num7}.", "Objednávka {num7}.",
    "Ordre {num6}.", "Tilaus {num7}.", "Comanda {num7}.", "Rendelés {num7}.",
    "Call {phone}.", "Room {num4}.", "Since {year}.",
]


def distractor(rng):
    return DISTRACTORS[rng.randrange(len(DISTRACTORS))].format(
        num4=number(rng, 4), num5=number(rng, 5), num6=number(rng, 6), num7=number(rng, 7),
        num8=number(rng, 8), ref=ref(rng), phone=phone(rng),
        year=str(rng.choice([2019, 2021, 2024, 2026])))


# The zero of each digit script codes may be written in.
ZEROS = {"arab": 0x0660, "persian": 0x06F0, "deva": 0x0966, "beng": 0x09E6,
         "thai": 0x0E50, "fullwidth": 0xFF10}


def in_script(code, script):
    """`code` with its ASCII digits written in `script`."""
    zero = ZEROS[script]
    return "".join(chr(zero + int(c)) if c.isascii() and c.isdigit() else c for c in code)


def messages(rng, positive, negative, count):
    """(subject, body, code or None) triples, about half with a code."""
    out = []
    for _ in range(count):
        if rng.random() < 0.55:
            template = rng.choice(positive)
            c = code(rng)
            script = template[2] if len(template) > 2 else ""
            if script and rng.random() < 0.35:
                c = in_script(code(rng, rng.choice(["d4", "d6", "d6", "d8"])), script)
            subject, body = fill(rng, template[:2], c)
            # Another number beside the code, before or after it.
            if rng.random() < 0.35:
                body = (body + rng.choice([" ", "\n"]) + distractor(rng)
                        if rng.random() < 0.6 else distractor(rng) + "\n" + body)
            out.append((*vary(rng, subject, body), c))
        else:
            out.append((*vary(rng, *fill(rng, rng.choice(negative), "")), None))
    return out
