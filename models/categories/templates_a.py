"""Made-up automated mail in English, Spanish, German and French: for each
kind of message (an order shipped, a sale, a new follower, a discussion
list's post…), ways its subject and preview might read, as
"subject || preview". English has four of each, the others two; data.py
trains on the even-numbered and holds out the odd-numbered ones.

{brand} is a shop's or service's name, {app} a social network's, {name} a
person's, {group} a discussion group's, {topic} what a thread is about,
{item} a product, {n} a small number, {pct} a percentage, {price} an
amount, {order} an order number, {day} a weekday and {last4} four digits.
"""

T = {
    # Transactions.
    "order": {
        "en": [
            "Your {brand} order #{order} is confirmed || Thanks for your order! We'll let you know when your {item} ships. Order total: {price}",
            "Order confirmation #{order} || Hi {name}, we've received your order and are getting it ready. Items: {item}",
            "Thanks for shopping with {brand} || Your order {order} has been placed. Estimated delivery: {day}.",
            "We got your order! || Order #{order} · {item} · Subtotal {price}. View or manage your order.",
        ],
        "es": [
            "Confirmación de tu pedido #{order} || ¡Gracias por tu compra! Te avisaremos cuando enviemos tu {item}. Total: {price}",
            "Hemos recibido tu pedido || Hola {name}, estamos preparando tu pedido {order}. Entrega estimada: {day}.",
        ],
        "de": [
            "Ihre Bestellung #{order} bei {brand} || Vielen Dank für Ihre Bestellung! Wir informieren Sie, sobald {item} versandt wird. Summe: {price}",
            "Bestellbestätigung {order} || Hallo {name}, wir haben Ihre Bestellung erhalten und bereiten sie vor.",
        ],
        "fr": [
            "Confirmation de votre commande n°{order} || Merci pour votre commande ! Nous vous préviendrons dès l'expédition de {item}. Total : {price}",
            "Nous avons bien reçu votre commande || Bonjour {name}, votre commande {order} est en préparation. Livraison estimée : {day}.",
        ],
    },
    "shipped": {
        "en": [
            "Your package is on its way || Good news! Your {item} has shipped and should arrive by {day}. Track your package.",
            "Shipped: your order #{order} || Your order has left our warehouse. Tracking number available in your account.",
            "Your {brand} order has shipped || It's on the way! Estimated delivery {day}.",
            "Out for delivery today || Your package from {brand} is out for delivery and will arrive today.",
        ],
        "es": [
            "Tu pedido está en camino || ¡Buenas noticias! Tu {item} ha sido enviado y llegará el {day}. Sigue tu envío.",
            "Pedido #{order} enviado || Tu pedido ya salió de nuestro almacén. Consulta el número de seguimiento.",
        ],
        "de": [
            "Ihr Paket ist unterwegs || Gute Nachrichten! Ihr {item} wurde verschickt und kommt voraussichtlich am {day}.",
            "Versandbestätigung zu Bestellung {order} || Ihre Bestellung hat unser Lager verlassen. Sendungsverfolgung im Konto.",
        ],
        "fr": [
            "Votre colis est en route || Bonne nouvelle ! Votre {item} a été expédié et arrivera {day}. Suivre mon colis.",
            "Commande n°{order} expédiée || Votre commande a quitté notre entrepôt. Numéro de suivi disponible.",
        ],
    },
    "delivered": {
        "en": [
            "Delivered: your {item} || Your package was delivered. Left at front door.",
            "Your order has been delivered || Order #{order} was delivered today at {n}:15 PM. How did we do?",
            "Package delivered || We delivered your package from {brand}. Not received? Let us know.",
            "Your delivery is complete || Your parcel was handed to a resident. Rate your delivery.",
        ],
        "es": [
            "Tu pedido ha sido entregado || Tu paquete fue entregado hoy. ¿No lo recibiste? Avísanos.",
            "Entregado: {item} || Hemos entregado tu pedido #{order}. Valora tu entrega.",
        ],
        "de": [
            "Ihr Paket wurde zugestellt || Ihre Sendung wurde heute zugestellt. Nicht erhalten? Melden Sie sich.",
            "Zugestellt: {item} || Bestellung {order} wurde geliefert. Wie zufrieden sind Sie?",
        ],
        "fr": [
            "Votre colis a été livré || Votre commande n°{order} a été livrée aujourd'hui. Pas reçu ? Contactez-nous.",
            "Livré : {item} || Nous avons livré votre colis {brand}. Évaluez votre livraison.",
        ],
    },
    "receipt": {
        "en": [
            "Your receipt from {brand} || Receipt #{order}. Amount paid: {price}. Payment method: card ending {last4}.",
            "Payment receipt || You paid {price} to {brand}. Thanks for your payment.",
            "Your {brand} invoice || Invoice {order} for {price} is attached. Paid on {day}.",
            "Receipt for your purchase || {item} — {price}. Keep this email for your records.",
        ],
        "es": [
            "Tu recibo de {brand} || Recibo n.º {order}. Importe pagado: {price}. Tarjeta terminada en {last4}.",
            "Factura de tu compra || Adjuntamos la factura {order} por {price}. Gracias por tu pago.",
        ],
        "de": [
            "Ihre Rechnung von {brand} || Rechnung {order} über {price}. Bezahlt mit Karte endend auf {last4}.",
            "Zahlungsbeleg || Sie haben {price} an {brand} gezahlt. Vielen Dank für Ihre Zahlung.",
        ],
        "fr": [
            "Votre reçu {brand} || Reçu n°{order}. Montant payé : {price}. Carte se terminant par {last4}.",
            "Facture de votre achat || Votre facture {order} de {price} est jointe. Merci pour votre paiement.",
        ],
    },
    "refund": {
        "en": [
            "Your refund has been processed || We've refunded {price} to your card ending {last4}. It may take 5-10 days.",
            "Refund for order #{order} || Your return was received and a refund of {price} is on its way.",
            "Return received || We got your return of {item}. Your refund will be issued shortly.",
            "We've issued your refund || {price} has been credited to your original payment method.",
        ],
        "es": [
            "Hemos procesado tu reembolso || Te hemos reembolsado {price} en tu tarjeta terminada en {last4}.",
            "Reembolso del pedido #{order} || Recibimos tu devolución de {item}. El reembolso está en camino.",
        ],
        "de": [
            "Ihre Rückerstattung wurde veranlasst || Wir haben {price} auf Ihre Karte endend auf {last4} erstattet.",
            "Rücksendung erhalten || Wir haben Ihre Rücksendung von {item} erhalten. Die Erstattung folgt in Kürze.",
        ],
        "fr": [
            "Votre remboursement a été effectué || Nous avons remboursé {price} sur votre carte se terminant par {last4}.",
            "Retour reçu || Nous avons reçu votre retour de {item}. Votre remboursement arrive bientôt.",
        ],
    },
    "booking": {
        "en": [
            "Your booking is confirmed || Reservation {order} in {city}, check-in {day}. We look forward to welcoming you.",
            "Your e-ticket and itinerary || Flight to {city} on {day}. Booking reference {order}.",
            "Booking confirmation: table for {n} || Your reservation at {brand} on {day} at 7:30 PM is confirmed.",
            "Your tickets for {day} || Order {order}: {n} tickets. Show this email at the entrance.",
        ],
        "es": [
            "Tu reserva está confirmada || Reserva {order} en {city}, entrada el {day}. ¡Te esperamos!",
            "Tu billete electrónico || Vuelo a {city} el {day}. Localizador {order}.",
        ],
        "de": [
            "Ihre Buchung ist bestätigt || Reservierung {order} in {city}, Anreise am {day}. Wir freuen uns auf Sie.",
            "Ihr Ticket und Reiseplan || Flug nach {city} am {day}. Buchungsnummer {order}.",
        ],
        "fr": [
            "Votre réservation est confirmée || Réservation {order} à {city}, arrivée le {day}. À bientôt !",
            "Votre billet électronique || Vol pour {city} le {day}. Référence de réservation {order}.",
        ],
    },
    # Updates.
    "security": {
        "en": [
            "Security alert: new sign-in || We noticed a new sign-in to your {brand} account from a new device. If this was you, you can ignore this email.",
            "Your password was changed || The password for your {brand} account was changed. If you didn't do this, secure your account.",
            "Unusual activity on your account || We blocked a sign-in attempt from {city}. Review your security settings.",
            "Two-step verification turned on || You've added an extra layer of security to your account.",
        ],
        "es": [
            "Alerta de seguridad: nuevo inicio de sesión || Detectamos un nuevo inicio de sesión en tu cuenta de {brand}. Si fuiste tú, ignora este correo.",
            "Tu contraseña ha cambiado || Se cambió la contraseña de tu cuenta. Si no fuiste tú, protege tu cuenta.",
        ],
        "de": [
            "Sicherheitshinweis: neue Anmeldung || Wir haben eine neue Anmeldung bei Ihrem {brand}-Konto festgestellt. Waren Sie das?",
            "Ihr Passwort wurde geändert || Das Passwort Ihres Kontos wurde geändert. Falls nicht Sie es waren, sichern Sie Ihr Konto.",
        ],
        "fr": [
            "Alerte de sécurité : nouvelle connexion || Nous avons détecté une nouvelle connexion à votre compte {brand}. Était-ce vous ?",
            "Votre mot de passe a été modifié || Le mot de passe de votre compte a été modifié. Si ce n'était pas vous, sécurisez votre compte.",
        ],
    },
    "statement": {
        "en": [
            "Your statement is ready || Your {brand} statement for the period ending {day} is now available online.",
            "Your monthly account summary || Balance: {price}. View your activity and statements.",
            "Low balance alert || Your available balance is below {price}. Account ending {last4}.",
            "Your annual tax documents are available || Your documents for the year are ready to download.",
        ],
        "es": [
            "Tu extracto está disponible || Ya puedes consultar el extracto de tu cuenta terminada en {last4}.",
            "Resumen mensual de tu cuenta || Saldo: {price}. Consulta tus movimientos.",
        ],
        "de": [
            "Ihr Kontoauszug ist verfügbar || Ihr Kontoauszug für Konto endend auf {last4} steht online bereit.",
            "Kontostand-Benachrichtigung || Ihr verfügbarer Saldo liegt unter {price}.",
        ],
        "fr": [
            "Votre relevé est disponible || Le relevé de votre compte se terminant par {last4} est disponible en ligne.",
            "Alerte solde || Votre solde disponible est inférieur à {price}.",
        ],
    },
    "bill": {
        "en": [
            "Your bill is ready || Your {brand} bill of {price} is due on {day}.",
            "Payment due reminder || A payment of {price} is due soon. Set up autopay to never miss a payment.",
            "Your electricity bill for this month || Amount due: {price}. Due date: {day}.",
            "Upcoming automatic payment || We'll charge {price} to your card ending {last4} on {day}.",
        ],
        "es": [
            "Tu factura ya está disponible || Tu factura de {brand} de {price} vence el {day}.",
            "Recordatorio de pago || Tienes un pago de {price} pendiente. Fecha de vencimiento: {day}.",
        ],
        "de": [
            "Ihre Rechnung ist da || Ihre {brand}-Rechnung über {price} ist am {day} fällig.",
            "Zahlungserinnerung || Eine Zahlung von {price} ist bald fällig. Richten Sie das Lastschriftverfahren ein.",
        ],
        "fr": [
            "Votre facture est disponible || Votre facture {brand} de {price} est à régler avant le {day}.",
            "Rappel d'échéance || Un paiement de {price} arrive à échéance le {day}.",
        ],
    },
    "reminder": {
        "en": [
            "Reminder: your appointment on {day} || This is a reminder of your appointment at {brand} on {day} at 10:00.",
            "Don't forget: {topic} tomorrow || Your event starts tomorrow. Add it to your calendar.",
            "Your library books are due soon || {n} items are due on {day}. Renew online.",
            "Reminder: complete your profile || You're almost done setting up your {brand} account.",
        ],
        "es": [
            "Recordatorio: tu cita del {day} || Te recordamos tu cita en {brand} el {day} a las 10:00.",
            "No olvides: {topic} mañana || Tu evento empieza mañana. Añádelo a tu calendario.",
        ],
        "de": [
            "Erinnerung: Ihr Termin am {day} || Wir erinnern Sie an Ihren Termin bei {brand} am {day} um 10:00 Uhr.",
            "Nicht vergessen: {topic} morgen || Ihre Veranstaltung beginnt morgen.",
        ],
        "fr": [
            "Rappel : votre rendez-vous du {day} || Nous vous rappelons votre rendez-vous chez {brand} le {day} à 10h00.",
            "N'oubliez pas : {topic} demain || Votre événement commence demain.",
        ],
    },
    "renewal": {
        "en": [
            "Your subscription will renew soon || Your {brand} plan renews on {day} for {price}. Manage your subscription.",
            "Your free trial ends in {n} days || After your trial, you'll be charged {price} per month.",
            "Your membership has been renewed || Thanks for staying with {brand}. Next billing date: {day}.",
            "Action required: update your payment method || We couldn't process your payment for your subscription.",
        ],
        "es": [
            "Tu suscripción se renovará pronto || Tu plan de {brand} se renueva el {day} por {price}.",
            "Tu prueba gratuita termina en {n} días || Después se cobrarán {price} al mes.",
        ],
        "de": [
            "Ihr Abonnement verlängert sich bald || Ihr {brand}-Abo verlängert sich am {day} für {price}.",
            "Ihre Testphase endet in {n} Tagen || Danach werden {price} pro Monat berechnet.",
        ],
        "fr": [
            "Votre abonnement sera bientôt renouvelé || Votre abonnement {brand} sera renouvelé le {day} pour {price}.",
            "Votre essai gratuit se termine dans {n} jours || Ensuite, {price} par mois vous seront facturés.",
        ],
    },
    "newsletter": {
        "en": [
            "The week in {topic} || This week: what changed, what's next, and three stories worth reading.",
            "{brand} Weekly #{n} || In this issue: a deep dive into {topic}, reader questions, and links.",
            "Your morning briefing || Here's what you need to know today. Read in your browser.",
            "Notes from the {brand} team || A look back at the month: updates, lessons, and what we're working on.",
        ],
        "es": [
            "La semana en {topic} || Esta semana: qué cambió, qué viene y tres historias que vale la pena leer.",
            "Boletín semanal de {brand} n.º {n} || En este número: {topic}, preguntas de lectores y enlaces.",
        ],
        "de": [
            "Die Woche in {topic} || Diese Woche: was sich geändert hat, was kommt, und drei lesenswerte Geschichten.",
            "{brand} Newsletter #{n} || In dieser Ausgabe: {topic}, Leserfragen und Links.",
        ],
        "fr": [
            "La semaine en {topic} || Cette semaine : ce qui a changé, ce qui arrive et trois articles à lire.",
            "La lettre de {brand} n°{n} || Dans ce numéro : {topic}, vos questions et nos liens.",
        ],
    },
    "notification": {
        "en": [
            "[{brand}] Build failed on main || The latest run of CI failed. View the logs.",
            "{name} assigned you a task || {topic} — due {day}. Open in {brand}.",
            "{name} shared a document with you || \"{topic}\" was shared with you. Open document.",
            "Your report is ready || The {topic} report you requested is ready to download.",
        ],
        "es": [
            "{name} te asignó una tarea || {topic} — vence el {day}. Ábrela en {brand}.",
            "{name} compartió un documento contigo || Se compartió contigo \"{topic}\". Abrir documento.",
        ],
        "de": [
            "{name} hat Ihnen eine Aufgabe zugewiesen || {topic} — fällig am {day}. In {brand} öffnen.",
            "{name} hat ein Dokument mit Ihnen geteilt || \"{topic}\" wurde mit Ihnen geteilt.",
        ],
        "fr": [
            "{name} vous a assigné une tâche || {topic} — échéance le {day}. Ouvrir dans {brand}.",
            "{name} a partagé un document avec vous || « {topic} » a été partagé avec vous.",
        ],
    },
    "welcome": {
        "en": [
            "Welcome to {brand}! || Thanks for signing up. Confirm your email address to get started.",
            "Please verify your email || Click the link below to verify your email address for {brand}.",
            "Your account has been created || You can now sign in to {brand} with this email address.",
            "Getting started with {brand} || Here are three things to try first.",
        ],
        "es": [
            "¡Bienvenido a {brand}! || Gracias por registrarte. Confirma tu correo para empezar.",
            "Verifica tu dirección de correo || Haz clic en el enlace para confirmar tu cuenta de {brand}.",
        ],
        "de": [
            "Willkommen bei {brand}! || Danke für Ihre Anmeldung. Bestätigen Sie Ihre E-Mail-Adresse.",
            "Bitte bestätigen Sie Ihre E-Mail-Adresse || Klicken Sie auf den Link, um Ihr {brand}-Konto zu aktivieren.",
        ],
        "fr": [
            "Bienvenue chez {brand} ! || Merci pour votre inscription. Confirmez votre adresse e-mail.",
            "Confirmez votre adresse e-mail || Cliquez sur le lien pour activer votre compte {brand}.",
        ],
    },
    "policy": {
        "en": [
            "We're updating our Terms of Service || Changes take effect on {day}. Here's what's changing.",
            "An update to our privacy policy || We've made our privacy policy clearer. No action is needed.",
            "Important changes to your account || Starting {day}, we're changing how {topic} works.",
            "Service update: scheduled maintenance || {brand} will be unavailable on {day} from 2:00 to 4:00 AM.",
        ],
        "es": [
            "Actualizamos nuestros términos del servicio || Los cambios entran en vigor el {day}.",
            "Actualización de nuestra política de privacidad || Hemos aclarado nuestra política. No tienes que hacer nada.",
        ],
        "de": [
            "Wir aktualisieren unsere Nutzungsbedingungen || Die Änderungen gelten ab {day}.",
            "Aktualisierung unserer Datenschutzerklärung || Sie müssen nichts tun.",
        ],
        "fr": [
            "Mise à jour de nos conditions d'utilisation || Les changements prennent effet le {day}.",
            "Mise à jour de notre politique de confidentialité || Aucune action n'est requise.",
        ],
    },
    # Promotions.
    "sale": {
        "en": [
            "Up to {pct}% off everything ends tonight || Our biggest sale of the season. Shop now before it's gone!",
            "The {brand} sale is ON 🔥 || Save up to {pct}% on {item} and more. Limited time only.",
            "Last chance: {pct}% off || Don't miss out — prices go back up at midnight.",
            "Black Friday starts now || Doorbusters, deals, and {pct}% off sitewide. Shop the sale.",
        ],
        "es": [
            "Hasta {pct}% de descuento: termina hoy || Nuestras mayores rebajas de la temporada. ¡Compra ya!",
            "Rebajas en {brand} 🔥 || Ahorra hasta un {pct}% en {item}. Solo por tiempo limitado.",
        ],
        "de": [
            "Bis zu {pct}% Rabatt – nur heute || Unser großer Sale. Jetzt shoppen, solange der Vorrat reicht!",
            "Der {brand} Sale ist da 🔥 || Spare bis zu {pct}% auf {item}. Nur für kurze Zeit.",
        ],
        "fr": [
            "Jusqu'à -{pct}% : dernier jour || Nos plus belles soldes de la saison. Profitez-en vite !",
            "Les soldes {brand} sont là 🔥 || Jusqu'à {pct}% de réduction sur {item}. Offre limitée.",
        ],
    },
    "code": {
        "en": [
            "Here's {pct}% off your next order || Use code SAVE{pct} at checkout. Expires {day}.",
            "A gift for you 🎁 || Enjoy {price} off with this exclusive coupon. Shop now.",
            "Your exclusive discount code inside || Members get {pct}% off this weekend with code VIP{pct}.",
            "Treat yourself: {price} off || Your voucher is waiting. Valid until {day}.",
        ],
        "es": [
            "Un {pct}% de descuento en tu próximo pedido || Usa el código AHORRA{pct} al pagar. Válido hasta el {day}.",
            "Un regalo para ti 🎁 || Disfruta de {price} de descuento con este cupón exclusivo.",
        ],
        "de": [
            "{pct}% Rabatt auf Ihre nächste Bestellung || Mit dem Gutscheincode SPAR{pct} an der Kasse. Gültig bis {day}.",
            "Ein Geschenk für Sie 🎁 || Sichern Sie sich {price} Rabatt mit diesem exklusiven Gutschein.",
        ],
        "fr": [
            "-{pct}% sur votre prochaine commande || Utilisez le code promo PROMO{pct}. Valable jusqu'au {day}.",
            "Un cadeau pour vous 🎁 || Profitez de {price} de réduction avec ce bon exclusif.",
        ],
    },
    "arrivals": {
        "en": [
            "New arrivals just dropped || Fresh styles for the season are here. Shop the new collection.",
            "Back in stock: {item} || The one you loved is back — but not for long.",
            "Meet the new {item} || Designed for everyday comfort. Available now at {brand}.",
            "Just in: our {topic} collection || Discover what's new this week.",
        ],
        "es": [
            "Novedades recién llegadas || Ya están aquí los nuevos estilos de temporada. Descubre la colección.",
            "De nuevo disponible: {item} || El que te encantó ha vuelto, pero por poco tiempo.",
        ],
        "de": [
            "Neu eingetroffen || Die neuen Styles der Saison sind da. Entdecken Sie die Kollektion.",
            "Wieder verfügbar: {item} || Ihr Lieblingsstück ist zurück – aber nicht lange.",
        ],
        "fr": [
            "Nouveautés : elles viennent d'arriver || Les nouveaux styles de la saison sont là. Découvrez la collection.",
            "De retour en stock : {item} || Celui que vous aimiez est de retour, mais pas pour longtemps.",
        ],
    },
    "freeship": {
        "en": [
            "Free shipping on everything this weekend || No minimum. Ends {day}.",
            "FREE delivery + a surprise gift || Order today and get free shipping on {item}.",
            "Your cart misses you || You left {item} in your cart. Complete your order with free shipping.",
            "Free returns, always || Shop {brand} risk-free. Free shipping over {price}.",
        ],
        "es": [
            "Envío gratis en todo este fin de semana || Sin pedido mínimo. Termina el {day}.",
            "Tu carrito te echa de menos || Dejaste {item} en tu carrito. Termina tu compra con envío gratis.",
        ],
        "de": [
            "Kostenloser Versand auf alles am Wochenende || Kein Mindestbestellwert. Endet am {day}.",
            "Ihr Warenkorb wartet || Sie haben {item} im Warenkorb gelassen. Jetzt versandkostenfrei bestellen.",
        ],
        "fr": [
            "Livraison gratuite sur tout ce week-end || Sans minimum d'achat. Jusqu'au {day}.",
            "Votre panier vous attend || Vous avez laissé {item} dans votre panier. Livraison gratuite.",
        ],
    },
    "loyalty": {
        "en": [
            "You have {n}00 points waiting || Redeem your rewards before they expire on {day}.",
            "Double points this week only || Earn 2x rewards on every purchase at {brand}.",
            "You've unlocked Gold status ✨ || Enjoy member-only perks and early access.",
            "Your rewards balance || You're {n}0 points away from a free {item}.",
        ],
        "es": [
            "Tienes {n}00 puntos esperando || Canjea tus recompensas antes del {day}.",
            "Puntos dobles solo esta semana || Gana el doble de recompensas en cada compra en {brand}.",
        ],
        "de": [
            "{n}00 Punkte warten auf Sie || Lösen Sie Ihre Prämien vor dem {day} ein.",
            "Doppelte Punkte nur diese Woche || Sammeln Sie bei jedem Einkauf bei {brand} doppelt.",
        ],
        "fr": [
            "Vous avez {n}00 points à utiliser || Profitez de vos récompenses avant le {day}.",
            "Points doublés cette semaine seulement || Gagnez deux fois plus de points chez {brand}.",
        ],
    },
    "event": {
        "en": [
            "Join our free webinar on {topic} || Register now: {day} at 11 AM. Seats are limited.",
            "You're invited: {brand} Live || Join us for product demos and special offers. Save your spot.",
            "Last call to register || Our {topic} workshop starts {day}. Don't miss it.",
            "Meet us in {city} || Come visit our pop-up store this weekend. Exclusive in-store deals.",
        ],
        "es": [
            "Únete a nuestro webinar gratuito sobre {topic} || Regístrate: {day} a las 11:00. Plazas limitadas.",
            "Estás invitado: {brand} en directo || Demostraciones y ofertas especiales. Reserva tu plaza.",
        ],
        "de": [
            "Kostenloses Webinar zu {topic} || Jetzt anmelden: {day} um 11 Uhr. Begrenzte Plätze.",
            "Sie sind eingeladen: {brand} Live || Produktvorführungen und Sonderangebote. Platz sichern.",
        ],
        "fr": [
            "Participez à notre webinar gratuit sur {topic} || Inscrivez-vous : {day} à 11h. Places limitées.",
            "Vous êtes invité : {brand} Live || Démonstrations et offres spéciales. Réservez votre place.",
        ],
    },
    "comeback": {
        "en": [
            "We miss you, {name} || It's been a while. Here's {pct}% off to welcome you back.",
            "Still thinking about it? || {item} is waiting for you. Plus, an offer just for you.",
            "Upgrade to {brand} Premium || Get more features for {price}/month. Try it free for {n} days.",
            "Recommended for you || Based on your browsing: {item} and more picks we think you'll love.",
        ],
        "es": [
            "Te echamos de menos, {name} || Ha pasado un tiempo. Aquí tienes un {pct}% de descuento para volver.",
            "Recomendado para ti || Según lo que has visto: {item} y más productos que te encantarán.",
        ],
        "de": [
            "Wir vermissen Sie, {name} || Es ist eine Weile her. Hier sind {pct}% Rabatt zur Begrüßung.",
            "Für Sie empfohlen || Passend zu Ihrem Einkauf: {item} und mehr.",
        ],
        "fr": [
            "Vous nous manquez, {name} || Ça fait longtemps. Voici -{pct}% pour votre retour.",
            "Recommandé pour vous || D'après vos visites : {item} et d'autres coups de cœur.",
        ],
    },
    # Social.
    "comment": {
        "en": [
            "{name} commented on your post || \"Love this! Where was it taken?\" Reply on {app}.",
            "{name} replied to your comment || See the conversation on {app}.",
            "New comments on your photo || {name} and {n} others commented on your photo.",
            "{name} commented on a post you're tagged in || View the post on {app}.",
        ],
        "es": [
            "{name} comentó tu publicación || \"¡Me encanta! ¿Dónde es?\" Responde en {app}.",
            "{name} respondió a tu comentario || Mira la conversación en {app}.",
        ],
        "de": [
            "{name} hat deinen Beitrag kommentiert || „Wie schön! Wo ist das?“ Antworte auf {app}.",
            "{name} hat auf deinen Kommentar geantwortet || Sieh dir die Unterhaltung auf {app} an.",
        ],
        "fr": [
            "{name} a commenté votre publication || « Superbe ! C'est où ? » Répondez sur {app}.",
            "{name} a répondu à votre commentaire || Voir la conversation sur {app}.",
        ],
    },
    "follow": {
        "en": [
            "{name} started following you || You have {n} new followers on {app}.",
            "You have new followers || {name} and {n} others are now following you.",
            "{name} is now following you on {app} || Follow back?",
            "People you may know || {name} is on {app}. Connect with {name}.",
        ],
        "es": [
            "{name} empezó a seguirte || Tienes {n} seguidores nuevos en {app}.",
            "Tienes nuevos seguidores || {name} y {n} personas más te siguen ahora.",
        ],
        "de": [
            "{name} folgt dir jetzt || Du hast {n} neue Follower auf {app}.",
            "Du hast neue Follower || {name} und {n} weitere folgen dir jetzt.",
        ],
        "fr": [
            "{name} vous suit désormais || Vous avez {n} nouveaux abonnés sur {app}.",
            "Vous avez de nouveaux abonnés || {name} et {n} autres personnes vous suivent.",
        ],
    },
    "message": {
        "en": [
            "{name} sent you a message || \"Hey! Are you coming tonight?\" Reply on {app}.",
            "You have {n} new messages || {name}: \"Did you see this?\"",
            "New message from {name} on {app} || Open {app} to read and reply.",
            "{name} messaged you || You have unread messages waiting.",
        ],
        "es": [
            "{name} te envió un mensaje || \"¡Hola! ¿Vienes esta noche?\" Responde en {app}.",
            "Tienes {n} mensajes nuevos || {name}: \"¿Viste esto?\"",
        ],
        "de": [
            "{name} hat dir eine Nachricht gesendet || „Hey! Kommst du heute Abend?“ Antworte auf {app}.",
            "Du hast {n} neue Nachrichten || {name}: „Hast du das gesehen?“",
        ],
        "fr": [
            "{name} vous a envoyé un message || « Salut ! Tu viens ce soir ? » Répondez sur {app}.",
            "Vous avez {n} nouveaux messages || {name} : « Tu as vu ça ? »",
        ],
    },
    "friend": {
        "en": [
            "{name} sent you a friend request || You have {n} mutual friends. Confirm or delete.",
            "{name} wants to connect || Accept {name}'s invitation to connect on {app}.",
            "You have a new match! 💘 || {name} likes you too. Say hi on {app}.",
            "{name} accepted your invitation || You're now connected on {app}.",
        ],
        "es": [
            "{name} te envió una solicitud de amistad || Tenéis {n} amigos en común. Confirmar o eliminar.",
            "{name} quiere conectar contigo || Acepta la invitación de {name} en {app}.",
        ],
        "de": [
            "{name} hat dir eine Freundschaftsanfrage gesendet || Ihr habt {n} gemeinsame Freunde.",
            "{name} möchte sich mit dir vernetzen || Nimm die Einladung auf {app} an.",
        ],
        "fr": [
            "{name} vous a envoyé une demande d'ami || Vous avez {n} amis en commun.",
            "{name} souhaite se connecter avec vous || Acceptez l'invitation sur {app}.",
        ],
    },
    "mention": {
        "en": [
            "{name} mentioned you in a comment || \"@you you have to see this\" — view on {app}.",
            "{name} tagged you in a photo || See the photo on {app}.",
            "You were mentioned in a post || {name} mentioned you. See what they said.",
            "{name} tagged you in {n} photos || Review tags on {app}.",
        ],
        "es": [
            "{name} te mencionó en un comentario || Mira lo que dijo en {app}.",
            "{name} te etiquetó en una foto || Mira la foto en {app}.",
        ],
        "de": [
            "{name} hat dich in einem Kommentar erwähnt || Sieh nach, was gesagt wurde, auf {app}.",
            "{name} hat dich in einem Foto markiert || Sieh dir das Foto auf {app} an.",
        ],
        "fr": [
            "{name} vous a mentionné dans un commentaire || Voyez ce qu'il a dit sur {app}.",
            "{name} vous a identifié sur une photo || Voir la photo sur {app}.",
        ],
    },
    "memories": {
        "en": [
            "Your memories from {n} years ago || Look back on this day with {name}.",
            "{name} and {n} others have birthdays today || Wish them a happy birthday on {app}.",
            "Your profile was viewed {n} times this week || See who's been looking at your profile.",
            "Trending in your network || {name} and others posted about {topic}.",
        ],
        "es": [
            "Tus recuerdos de hace {n} años || Recuerda este día con {name}.",
            "Tu perfil recibió {n} visitas esta semana || Mira quién ha visto tu perfil.",
        ],
        "de": [
            "Deine Erinnerungen von vor {n} Jahren || Blick zurück auf diesen Tag mit {name}.",
            "Dein Profil wurde diese Woche {n} Mal angesehen || Sieh nach, wer dein Profil besucht hat.",
        ],
        "fr": [
            "Vos souvenirs d'il y a {n} ans || Revivez cette journée avec {name}.",
            "Votre profil a été consulté {n} fois cette semaine || Découvrez qui a consulté votre profil.",
        ],
    },
    "like": {
        "en": [
            "{name} liked your photo || {name} and {n} others liked your post.",
            "Your post is getting attention || {n}0 people reacted to your post on {app}.",
            "{name} reacted to your story || See all reactions on {app}.",
            "{name} liked your comment || \"So true!\" — on {app}",
        ],
        "es": [
            "A {name} le gustó tu foto || A {name} y a {n} personas más les gustó tu publicación.",
            "Tu publicación está teniendo éxito || {n}0 personas reaccionaron a tu publicación.",
        ],
        "de": [
            "{name} gefällt dein Foto || {name} und {n} weiteren gefällt dein Beitrag.",
            "Dein Beitrag kommt gut an || {n}0 Personen haben auf deinen Beitrag reagiert.",
        ],
        "fr": [
            "{name} aime votre photo || {name} et {n} autres personnes aiment votre publication.",
            "Votre publication fait réagir || {n}0 personnes ont réagi à votre publication.",
        ],
    },
    # Forums.
    "post": {
        "en": [
            "Re: [{group}] {topic} || I ran into the same problem last week. Have you tried updating to the latest version? On {day}, {name} wrote:",
            "[{group}] Question about {topic} || Hi all, does anyone know how to handle this? Any pointers appreciated. Thanks, {name}",
            "Re: {topic} || +1, this would be really useful. I'd be happy to help test it.",
            "[{group}] Proposal: {topic} || Hello everyone, I'd like to propose a change. Feedback welcome before {day}.",
        ],
        "es": [
            "Re: [{group}] {topic} || Me pasó lo mismo la semana pasada. ¿Probaste a actualizar a la última versión? El {day}, {name} escribió:",
            "[{group}] Pregunta sobre {topic} || Hola a todos, ¿alguien sabe cómo resolver esto? Gracias, {name}",
        ],
        "de": [
            "Re: [{group}] {topic} || Ich hatte letzte Woche dasselbe Problem. Hast du schon auf die neueste Version aktualisiert? Am {day} schrieb {name}:",
            "[{group}] Frage zu {topic} || Hallo zusammen, weiß jemand, wie man das löst? Danke, {name}",
        ],
        "fr": [
            "Re: [{group}] {topic} || J'ai eu le même problème la semaine dernière. As-tu essayé la dernière version ? Le {day}, {name} a écrit :",
            "[{group}] Question sur {topic} || Bonjour à tous, quelqu'un sait-il comment faire ? Merci, {name}",
        ],
    },
    "digest": {
        "en": [
            "Digest for {group} - {n} updates in {n} topics || {topic} - {n} messages. You received this message because you are subscribed to the group.",
            "[{group}] Daily digest || Today's topics: {topic}; Meeting notes; Welcome new members.",
            "{group} weekly summary || Top discussions this week: {topic} and {n} more.",
            "Your {group} digest || {n} new posts since your last visit.",
        ],
        "es": [
            "Resumen de {group}: {n} novedades en {n} temas || {topic}: {n} mensajes. Recibiste este mensaje porque estás suscrito al grupo.",
            "[{group}] Resumen diario || Temas de hoy: {topic}; Actas de la reunión.",
        ],
        "de": [
            "Zusammenfassung für {group} – {n} neue Beiträge in {n} Themen || {topic}: {n} Nachrichten. Sie erhalten diese Nachricht, weil Sie die Gruppe abonniert haben.",
            "[{group}] Tägliche Zusammenfassung || Themen heute: {topic}; Protokoll.",
        ],
        "fr": [
            "Résumé de {group} : {n} messages dans {n} sujets || {topic} : {n} messages. Vous recevez ce message car vous êtes abonné au groupe.",
            "[{group}] Résumé quotidien || Sujets du jour : {topic} ; Compte rendu.",
        ],
    },
    "topic": {
        "en": [
            "[{group}] New topic: {topic} || {name} started a new discussion. Reply to this email to respond.",
            "[{group}] {name} replied to \"{topic}\" || Reply above this line to post to the forum.",
            "New post in {topic} || {name} posted in a thread you're watching.",
            "[{group}] Announcement: {topic} || Hi everyone, quick update from the organizers.",
        ],
        "es": [
            "[{group}] Nuevo tema: {topic} || {name} abrió una discusión. Responde a este correo para participar.",
            "[{group}] {name} respondió a \"{topic}\" || Responde encima de esta línea para publicar en el foro.",
        ],
        "de": [
            "[{group}] Neues Thema: {topic} || {name} hat eine Diskussion gestartet. Antworte auf diese E-Mail.",
            "[{group}] {name} hat auf „{topic}“ geantwortet || Antworte oberhalb dieser Zeile, um im Forum zu posten.",
        ],
        "fr": [
            "[{group}] Nouveau sujet : {topic} || {name} a lancé une discussion. Répondez à cet e-mail pour participer.",
            "[{group}] {name} a répondu à « {topic} » || Répondez au-dessus de cette ligne pour publier sur le forum.",
        ],
    },
}
