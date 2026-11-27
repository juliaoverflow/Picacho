"""Made-up mail for Travel, Finance and Newsletters, in the same 8 languages
and form as templates_a.py ("subject || preview"; English four ways, the
others two). With the bookings, statements, bills and newsletters of
templates_a.py and templates_b.py, which data.py counts in these
categories too."""

T = {
    "flight": {
        "en": [
            "Check-in is now open for your flight to {city} || Check in online and get your boarding pass. Departure {day} at 08:40.",
            "Your boarding pass || Flight to {city} · Gate B{n} · Boarding 07:55. Have a great trip!",
            "Flight delayed: new departure time || Your flight to {city} on {day} is now departing at 14:20. We apologize for the delay.",
            "Your trip to {city} is coming up || Your itinerary, hotel details and car rental are all in one place.",
        ],
        "es": [
            "Ya puedes hacer el check-in de tu vuelo a {city} || Haz el check-in online y descarga tu tarjeta de embarque. Salida el {day} a las 08:40.",
            "Cambio en tu vuelo || Tu vuelo a {city} del {day} sale ahora a las 14:20. Disculpa las molestias.",
        ],
        "de": [
            "Der Online-Check-in für Ihren Flug nach {city} ist geöffnet || Checken Sie jetzt ein und erhalten Sie Ihre Bordkarte. Abflug am {day} um 08:40.",
            "Flugänderung: neue Abflugzeit || Ihr Flug nach {city} am {day} startet jetzt um 14:20 Uhr.",
        ],
        "fr": [
            "L'enregistrement de votre vol pour {city} est ouvert || Enregistrez-vous en ligne et obtenez votre carte d'embarquement. Départ {day} à 08h40.",
            "Votre vol a été retardé || Votre vol pour {city} du {day} partira désormais à 14h20.",
        ],
        "pt": [
            "O check-in do seu voo para {city} está aberto || Faça o check-in online e baixe o cartão de embarque. Partida {day} às 08:40.",
            "Alteração no seu voo || Seu voo para {city} em {day} agora parte às 14:20.",
        ],
        "it": [
            "Il check-in per il tuo volo per {city} è aperto || Fai il check-in online e scarica la carta d'imbarco. Partenza {day} alle 08:40.",
            "Il tuo volo è in ritardo || Il volo per {city} di {day} partirà alle 14:20.",
        ],
        "nl": [
            "Inchecken voor je vlucht naar {city} is geopend || Check online in en download je instapkaart. Vertrek {day} om 08:40.",
            "Je vlucht is vertraagd || Je vlucht naar {city} op {day} vertrekt nu om 14:20.",
        ],
        "pl": [
            "Odprawa online na lot do {city} jest otwarta || Odpraw się online i pobierz kartę pokładową. Wylot {day} o 08:40.",
            "Zmiana godziny lotu || Twój lot do {city} w {day} wylatuje teraz o 14:20.",
        ],
    },
    "banking": {
        "en": [
            "You received {price} || {name} sent you {price}. The money is in your account.",
            "Transaction alert: card ending {last4} || A purchase of {price} was made at {brand}. Not you? Freeze your card.",
            "Your direct deposit has arrived || A deposit of {price} was added to your account ending {last4}.",
            "Your credit card payment is due {day} || Minimum payment: {price}. Pay now to avoid a late fee.",
        ],
        "es": [
            "Has recibido {price} || {name} te ha enviado {price}. El dinero ya está en tu cuenta.",
            "Aviso de movimiento: tarjeta terminada en {last4} || Se ha realizado un cargo de {price} en {brand}.",
        ],
        "de": [
            "Sie haben {price} erhalten || {name} hat Ihnen {price} überwiesen. Das Geld ist auf Ihrem Konto.",
            "Umsatz mit Karte endend auf {last4} || Eine Zahlung über {price} bei {brand} wurde belastet.",
        ],
        "fr": [
            "Vous avez reçu {price} || {name} vous a envoyé {price}. L'argent est sur votre compte.",
            "Alerte opération : carte se terminant par {last4} || Un paiement de {price} chez {brand} a été débité.",
        ],
        "pt": [
            "Você recebeu {price} || {name} enviou {price} para você via Pix. O dinheiro já está na conta.",
            "Compra aprovada no cartão final {last4} || Compra de {price} em {brand} aprovada.",
        ],
        "it": [
            "Hai ricevuto {price} || {name} ti ha inviato {price} con un bonifico.",
            "Avviso movimento: carta che termina con {last4} || Addebito di {price} presso {brand}.",
        ],
        "nl": [
            "Je hebt {price} ontvangen || {name} heeft je {price} overgemaakt. Het geld staat op je rekening.",
            "Transactiemelding: kaart eindigend op {last4} || Er is {price} afgeschreven bij {brand}.",
        ],
        "pl": [
            "Otrzymałeś {price} || {name} wysłał Ci przelew na {price}. Środki są na koncie.",
            "Powiadomienie o transakcji: karta kończąca się na {last4} || Obciążenie {price} w {brand}.",
        ],
    },
    "issue": {
        "en": [
            "Issue #{n}: {topic} || In this issue: why {topic} matters, a reader question, and what I'm reading. Read online.",
            "{brand} Daily: {topic} || Good morning. Here's today's briefing in five minutes.",
            "The {brand} Dispatch — {day} edition || This week's roundup: three essays on {topic} and links worth your time.",
            "A letter on {topic} || I've been thinking about {topic} a lot lately. Here's where I've landed.",
        ],
        "es": [
            "Número {n}: {topic} || En este número: por qué importa {topic}, una pregunta de un lector y lo que estoy leyendo.",
            "El resumen de {brand} del {day} || Buenos días. Esto es lo que necesitas saber hoy.",
        ],
        "de": [
            "Ausgabe {n}: {topic} || In dieser Ausgabe: warum {topic} wichtig ist, eine Leserfrage und meine Lesetipps.",
            "{brand} am Morgen – {day} || Guten Morgen. Das Wichtigste des Tages in fünf Minuten.",
        ],
        "fr": [
            "Numéro {n} : {topic} || Dans ce numéro : pourquoi {topic} compte, une question de lecteur et mes lectures.",
            "La lettre de {brand} du {day} || Bonjour. L'essentiel de l'actualité en cinq minutes.",
        ],
        "pt": [
            "Edição {n}: {topic} || Nesta edição: por que {topic} importa, uma pergunta de leitor e o que estou lendo.",
            "{brand} Diário — {day} || Bom dia. O resumo do dia em cinco minutos.",
        ],
        "it": [
            "Numero {n}: {topic} || In questo numero: perché {topic} conta, una domanda dei lettori e cosa sto leggendo.",
            "La rassegna di {brand} di {day} || Buongiorno. Le notizie del giorno in cinque minuti.",
        ],
        "nl": [
            "Editie {n}: {topic} || In deze editie: waarom {topic} ertoe doet, een lezersvraag en wat ik lees.",
            "{brand} Ochtendnieuws – {day} || Goedemorgen. Het belangrijkste nieuws in vijf minuten.",
        ],
        "pl": [
            "Wydanie {n}: {topic} || W tym wydaniu: dlaczego {topic} ma znaczenie, pytanie od czytelnika i co czytam.",
            "Poranny przegląd {brand} – {day} || Dzień dobry. Najważniejsze wiadomości w pięć minut.",
        ],
    },
}

T.update({
    "account": {
        "en": [
            "Low balance alert || Your available balance in account ending {last4} is below {price}.",
            "Your tax documents are ready || Your annual tax forms are now available to download.",
            "Autopay scheduled || We'll pay {price} from your checking account on {day}. No action needed.",
            "Your monthly investment summary || Your portfolio changed this month. Dividends paid: {price}.",
        ],
        "es": [
            "Aviso de saldo bajo || El saldo disponible de tu cuenta terminada en {last4} es inferior a {price}.",
            "Tus documentos fiscales están disponibles || Ya puedes descargar tu certificado de retenciones e impuestos.",
        ],
        "de": [
            "Hinweis: niedriger Kontostand || Ihr Kontostand auf dem Konto endend auf {last4} liegt unter {price}.",
            "Ihre Steuerbescheinigung ist da || Ihre jährliche Steuerbescheinigung steht zum Download bereit.",
        ],
        "fr": [
            "Alerte solde bas || Le solde de votre compte se terminant par {last4} est inférieur à {price}.",
            "Vos documents fiscaux sont disponibles || Votre imprimé fiscal unique est prêt à être téléchargé.",
        ],
        "pt": [
            "Alerta de saldo baixo || O saldo da sua conta final {last4} está abaixo de {price}.",
            "Seu informe de rendimentos está disponível || Baixe seu informe para a declaração de imposto.",
        ],
        "it": [
            "Avviso saldo basso || Il saldo del conto che termina con {last4} è inferiore a {price}.",
            "La tua certificazione fiscale è disponibile || Scarica la certificazione per la dichiarazione dei redditi.",
        ],
        "nl": [
            "Melding laag saldo || Het saldo van je rekening eindigend op {last4} is lager dan {price}.",
            "Je jaaroverzicht staat klaar || Download je jaaroverzicht voor je belastingaangifte.",
        ],
        "pl": [
            "Niskie saldo na koncie || Saldo konta kończącego się na {last4} spadło poniżej {price}.",
            "Twój PIT jest gotowy || Pobierz roczne zestawienie do zeznania podatkowego.",
        ],
    },
    "money": {
        "en": [
            "Money received || {name} paid you {price} for \"dinner\". Transfer to your bank anytime.",
            "Your transfer is on its way || {price} to {name} will arrive in 1–2 business days.",
            "Payment received — thank you || We received your payment of {price} for account ending {last4}.",
            "Your credit score was updated || Your score changed this month. See what affected it.",
        ],
        "es": [
            "Dinero recibido || {name} te ha pagado {price}. Transfiere a tu banco cuando quieras.",
            "Tu transferencia está en camino || Los {price} para {name} llegarán en 1 o 2 días hábiles.",
        ],
        "de": [
            "Geld erhalten || {name} hat dir {price} gesendet. Überweise es jederzeit auf dein Bankkonto.",
            "Ihre Überweisung ist unterwegs || {price} an {name} kommen in 1–2 Werktagen an.",
        ],
        "fr": [
            "Argent reçu || {name} vous a payé {price}. Virez-le sur votre compte bancaire à tout moment.",
            "Votre virement est en cours || {price} à {name} arriveront sous 1 à 2 jours ouvrés.",
        ],
        "pt": [
            "Dinheiro recebido || {name} pagou {price} para você. Transfira para o seu banco quando quiser.",
            "Sua transferência está a caminho || {price} para {name} chegam em 1 a 2 dias úteis.",
        ],
        "it": [
            "Denaro ricevuto || {name} ti ha pagato {price}. Trasferiscilo sul tuo conto quando vuoi.",
            "Il tuo bonifico è in arrivo || {price} a {name} arriveranno in 1–2 giorni lavorativi.",
        ],
        "nl": [
            "Geld ontvangen || {name} heeft je {price} betaald. Maak het over naar je bank wanneer je wilt.",
            "Je overboeking is onderweg || {price} aan {name} komt binnen 1–2 werkdagen aan.",
        ],
        "pl": [
            "Otrzymano pieniądze || {name} zapłacił Ci {price}. Przelej je na konto, kiedy chcesz.",
            "Twój przelew jest w drodze || {price} dla {name} dotrze w 1–2 dni robocze.",
        ],
    },
})
