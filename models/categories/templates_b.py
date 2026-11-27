"""Made-up automated mail in Portuguese, Italian, Dutch and Polish, as in
templates_a.py: two ways each kind of message might read, as
"subject || preview"."""

T = {
    "order": {
        "pt": [
            "Confirmação do seu pedido #{order} || Obrigado pela sua compra! Avisaremos quando {item} for enviado. Total: {price}",
            "Recebemos o seu pedido || Olá {name}, estamos preparando o pedido {order}. Entrega prevista: {day}.",
        ],
        "it": [
            "Conferma del tuo ordine n. {order} || Grazie per il tuo acquisto! Ti avviseremo quando {item} sarà spedito. Totale: {price}",
            "Abbiamo ricevuto il tuo ordine || Ciao {name}, stiamo preparando l'ordine {order}. Consegna prevista: {day}.",
        ],
        "nl": [
            "Bevestiging van je bestelling #{order} || Bedankt voor je bestelling! We laten het je weten zodra {item} is verzonden. Totaal: {price}",
            "We hebben je bestelling ontvangen || Hoi {name}, we maken bestelling {order} klaar. Verwachte levering: {day}.",
        ],
        "pl": [
            "Potwierdzenie zamówienia nr {order} || Dziękujemy za zakupy! Damy znać, gdy {item} zostanie wysłany. Razem: {price}",
            "Otrzymaliśmy Twoje zamówienie || Cześć {name}, przygotowujemy zamówienie {order}. Przewidywana dostawa: {day}.",
        ],
    },
    "shipped": {
        "pt": [
            "Seu pedido está a caminho || Boa notícia! {item} foi enviado e deve chegar {day}. Acompanhe a entrega.",
            "Pedido #{order} enviado || Seu pedido saiu do nosso armazém. Código de rastreio disponível.",
        ],
        "it": [
            "Il tuo pacco è in viaggio || Buone notizie! {item} è stato spedito e arriverà {day}. Traccia la spedizione.",
            "Ordine n. {order} spedito || Il tuo ordine ha lasciato il magazzino. Numero di tracciamento disponibile.",
        ],
        "nl": [
            "Je pakket is onderweg || Goed nieuws! {item} is verzonden en komt naar verwachting {day} aan.",
            "Bestelling #{order} verzonden || Je bestelling heeft ons magazijn verlaten. Volg je pakket.",
        ],
        "pl": [
            "Twoja paczka jest w drodze || Dobra wiadomość! {item} został wysłany i dotrze {day}. Śledź przesyłkę.",
            "Zamówienie nr {order} wysłane || Twoje zamówienie opuściło magazyn. Numer przesyłki w koncie.",
        ],
    },
    "delivered": {
        "pt": [
            "Seu pedido foi entregue || Seu pacote foi entregue hoje. Não recebeu? Fale conosco.",
            "Entregue: {item} || Entregamos o pedido #{order}. Avalie a entrega.",
        ],
        "it": [
            "Il tuo ordine è stato consegnato || Il pacco è stato consegnato oggi. Non l'hai ricevuto? Contattaci.",
            "Consegnato: {item} || Abbiamo consegnato l'ordine n. {order}. Valuta la consegna.",
        ],
        "nl": [
            "Je pakket is bezorgd || Je bestelling is vandaag bezorgd. Niet ontvangen? Laat het ons weten.",
            "Bezorgd: {item} || Bestelling #{order} is afgeleverd. Hoe hebben we het gedaan?",
        ],
        "pl": [
            "Twoja paczka została doręczona || Przesyłka została dziś dostarczona. Nie dotarła? Daj nam znać.",
            "Doręczono: {item} || Zamówienie nr {order} zostało dostarczone. Oceń dostawę.",
        ],
    },
    "receipt": {
        "pt": [
            "Seu recibo da {brand} || Recibo n.º {order}. Valor pago: {price}. Cartão final {last4}.",
            "Nota fiscal da sua compra || Segue a nota fiscal {order} no valor de {price}. Obrigado pelo pagamento.",
        ],
        "it": [
            "La tua ricevuta di {brand} || Ricevuta n. {order}. Importo pagato: {price}. Carta che termina con {last4}.",
            "Fattura del tuo acquisto || In allegato la fattura {order} di {price}. Grazie per il pagamento.",
        ],
        "nl": [
            "Je betaalbewijs van {brand} || Bon nr. {order}. Betaald bedrag: {price}. Kaart eindigend op {last4}.",
            "Factuur van je aankoop || Bijgevoegd vind je factuur {order} van {price}. Bedankt voor je betaling.",
        ],
        "pl": [
            "Twój paragon od {brand} || Paragon nr {order}. Zapłacono: {price}. Karta kończąca się na {last4}.",
            "Faktura za zakup || W załączniku faktura {order} na kwotę {price}. Dziękujemy za płatność.",
        ],
    },
    "refund": {
        "pt": [
            "Seu reembolso foi processado || Reembolsamos {price} no cartão final {last4}.",
            "Devolução recebida || Recebemos a devolução de {item}. O reembolso será feito em breve.",
        ],
        "it": [
            "Il tuo rimborso è stato elaborato || Abbiamo rimborsato {price} sulla carta che termina con {last4}.",
            "Reso ricevuto || Abbiamo ricevuto il reso di {item}. Il rimborso arriverà a breve.",
        ],
        "nl": [
            "Je terugbetaling is verwerkt || We hebben {price} teruggestort op je kaart eindigend op {last4}.",
            "Retour ontvangen || We hebben je retour van {item} ontvangen. De terugbetaling volgt snel.",
        ],
        "pl": [
            "Zwrot pieniędzy został zrealizowany || Zwróciliśmy {price} na kartę kończącą się na {last4}.",
            "Otrzymaliśmy zwrot || Otrzymaliśmy zwrot {item}. Pieniądze wkrótce wrócą na konto.",
        ],
    },
    "booking": {
        "pt": [
            "Sua reserva está confirmada || Reserva {order} em {city}, check-in {day}. Esperamos por você.",
            "Seu bilhete eletrônico || Voo para {city} em {day}. Código da reserva {order}.",
        ],
        "it": [
            "La tua prenotazione è confermata || Prenotazione {order} a {city}, arrivo {day}. Ti aspettiamo.",
            "Il tuo biglietto elettronico || Volo per {city} il {day}. Codice di prenotazione {order}.",
        ],
        "nl": [
            "Je reservering is bevestigd || Reservering {order} in {city}, inchecken {day}. Tot dan!",
            "Je e-ticket en reisschema || Vlucht naar {city} op {day}. Boekingsnummer {order}.",
        ],
        "pl": [
            "Twoja rezerwacja jest potwierdzona || Rezerwacja {order} w {city}, zameldowanie {day}.",
            "Twój bilet elektroniczny || Lot do {city} w {day}. Numer rezerwacji {order}.",
        ],
    },
    "security": {
        "pt": [
            "Alerta de segurança: novo acesso || Detectamos um novo acesso à sua conta {brand}. Se foi você, ignore este e-mail.",
            "Sua senha foi alterada || A senha da sua conta foi alterada. Se não foi você, proteja sua conta.",
        ],
        "it": [
            "Avviso di sicurezza: nuovo accesso || Abbiamo rilevato un nuovo accesso al tuo account {brand}. Eri tu?",
            "La tua password è stata modificata || La password del tuo account è stata cambiata. Se non sei stato tu, proteggi l'account.",
        ],
        "nl": [
            "Beveiligingsmelding: nieuwe aanmelding || We zagen een nieuwe aanmelding op je {brand}-account. Was jij dit?",
            "Je wachtwoord is gewijzigd || Het wachtwoord van je account is gewijzigd. Was jij het niet? Beveilig je account.",
        ],
        "pl": [
            "Alert bezpieczeństwa: nowe logowanie || Wykryliśmy nowe logowanie na Twoje konto {brand}. Czy to Ty?",
            "Twoje hasło zostało zmienione || Hasło do Twojego konta zostało zmienione. Jeśli to nie Ty, zabezpiecz konto.",
        ],
    },
    "statement": {
        "pt": [
            "Seu extrato está disponível || O extrato da conta final {last4} já está disponível online.",
            "Alerta de saldo || Seu saldo disponível está abaixo de {price}.",
        ],
        "it": [
            "Il tuo estratto conto è disponibile || L'estratto conto del conto che termina con {last4} è online.",
            "Avviso saldo || Il tuo saldo disponibile è inferiore a {price}.",
        ],
        "nl": [
            "Je rekeningafschrift staat klaar || Het afschrift van rekening eindigend op {last4} staat online.",
            "Saldomelding || Je beschikbare saldo is lager dan {price}.",
        ],
        "pl": [
            "Twój wyciąg jest dostępny || Wyciąg z konta kończącego się na {last4} jest dostępny online.",
            "Powiadomienie o saldzie || Twoje dostępne saldo spadło poniżej {price}.",
        ],
    },
    "bill": {
        "pt": [
            "Sua fatura está disponível || Sua fatura {brand} de {price} vence em {day}.",
            "Lembrete de vencimento || Você tem um pagamento de {price} com vencimento em {day}.",
        ],
        "it": [
            "La tua bolletta è disponibile || La bolletta {brand} di {price} scade il {day}.",
            "Promemoria di pagamento || Un pagamento di {price} è in scadenza il {day}.",
        ],
        "nl": [
            "Je factuur staat klaar || Je {brand}-factuur van {price} moet uiterlijk {day} betaald zijn.",
            "Betalingsherinnering || Een betaling van {price} vervalt op {day}.",
        ],
        "pl": [
            "Twój rachunek jest gotowy || Rachunek {brand} na kwotę {price}, termin płatności {day}.",
            "Przypomnienie o płatności || Płatność {price} przypada na {day}.",
        ],
    },
    "reminder": {
        "pt": [
            "Lembrete: sua consulta em {day} || Lembramos da sua consulta na {brand} em {day} às 10:00.",
            "Não se esqueça: {topic} amanhã || Seu evento começa amanhã.",
        ],
        "it": [
            "Promemoria: il tuo appuntamento di {day} || Ti ricordiamo l'appuntamento presso {brand} {day} alle 10:00.",
            "Non dimenticare: {topic} domani || Il tuo evento inizia domani.",
        ],
        "nl": [
            "Herinnering: je afspraak op {day} || We herinneren je aan je afspraak bij {brand} op {day} om 10:00.",
            "Niet vergeten: {topic} morgen || Je evenement begint morgen.",
        ],
        "pl": [
            "Przypomnienie: wizyta w {day} || Przypominamy o wizycie w {brand} w {day} o 10:00.",
            "Nie zapomnij: {topic} jutro || Twoje wydarzenie zaczyna się jutro.",
        ],
    },
    "renewal": {
        "pt": [
            "Sua assinatura será renovada em breve || Seu plano {brand} será renovado em {day} por {price}.",
            "Seu teste gratuito termina em {n} dias || Depois, serão cobrados {price} por mês.",
        ],
        "it": [
            "Il tuo abbonamento si rinnoverà presto || Il piano {brand} si rinnova il {day} a {price}.",
            "La prova gratuita termina tra {n} giorni || Poi ti verranno addebitati {price} al mese.",
        ],
        "nl": [
            "Je abonnement wordt binnenkort verlengd || Je {brand}-abonnement wordt op {day} verlengd voor {price}.",
            "Je gratis proefperiode eindigt over {n} dagen || Daarna betaal je {price} per maand.",
        ],
        "pl": [
            "Twoja subskrypcja wkrótce się odnowi || Plan {brand} odnowi się {day} za {price}.",
            "Okres próbny kończy się za {n} dni || Następnie pobierzemy {price} miesięcznie.",
        ],
    },
    "newsletter": {
        "pt": [
            "A semana em {topic} || Nesta semana: o que mudou, o que vem aí e três histórias para ler.",
            "Newsletter {brand} n.º {n} || Nesta edição: {topic}, perguntas dos leitores e links.",
        ],
        "it": [
            "La settimana in {topic} || Questa settimana: cosa è cambiato, cosa arriva e tre storie da leggere.",
            "Newsletter {brand} n. {n} || In questo numero: {topic}, domande dei lettori e link.",
        ],
        "nl": [
            "De week in {topic} || Deze week: wat er veranderde, wat eraan komt en drie verhalen om te lezen.",
            "{brand} nieuwsbrief #{n} || In deze editie: {topic}, lezersvragen en links.",
        ],
        "pl": [
            "Tydzień w {topic} || W tym tygodniu: co się zmieniło, co nas czeka i trzy teksty do przeczytania.",
            "Newsletter {brand} nr {n} || W tym wydaniu: {topic}, pytania czytelników i linki.",
        ],
    },
    "notification": {
        "pt": [
            "{name} atribuiu uma tarefa a você || {topic} — prazo {day}. Abrir no {brand}.",
            "{name} compartilhou um documento com você || \"{topic}\" foi compartilhado com você.",
        ],
        "it": [
            "{name} ti ha assegnato un'attività || {topic} — scadenza {day}. Apri in {brand}.",
            "{name} ha condiviso un documento con te || \"{topic}\" è stato condiviso con te.",
        ],
        "nl": [
            "{name} heeft je een taak toegewezen || {topic} — deadline {day}. Openen in {brand}.",
            "{name} heeft een document met je gedeeld || \"{topic}\" is met je gedeeld.",
        ],
        "pl": [
            "{name} przypisał Ci zadanie || {topic} — termin {day}. Otwórz w {brand}.",
            "{name} udostępnił Ci dokument || Udostępniono Ci \"{topic}\".",
        ],
    },
    "welcome": {
        "pt": [
            "Bem-vindo à {brand}! || Obrigado por se cadastrar. Confirme seu e-mail para começar.",
            "Confirme seu endereço de e-mail || Clique no link para ativar sua conta {brand}.",
        ],
        "it": [
            "Benvenuto in {brand}! || Grazie per l'iscrizione. Conferma il tuo indirizzo e-mail.",
            "Conferma il tuo indirizzo e-mail || Fai clic sul link per attivare il tuo account {brand}.",
        ],
        "nl": [
            "Welkom bij {brand}! || Bedankt voor je aanmelding. Bevestig je e-mailadres om te beginnen.",
            "Bevestig je e-mailadres || Klik op de link om je {brand}-account te activeren.",
        ],
        "pl": [
            "Witaj w {brand}! || Dziękujemy za rejestrację. Potwierdź adres e-mail, aby zacząć.",
            "Potwierdź swój adres e-mail || Kliknij link, aby aktywować konto {brand}.",
        ],
    },
    "policy": {
        "pt": [
            "Estamos atualizando nossos Termos de Uso || As mudanças entram em vigor em {day}.",
            "Atualização da nossa política de privacidade || Nenhuma ação é necessária.",
        ],
        "it": [
            "Aggiorniamo i nostri Termini di servizio || Le modifiche entrano in vigore il {day}.",
            "Aggiornamento dell'informativa sulla privacy || Non è richiesta alcuna azione.",
        ],
        "nl": [
            "We passen onze voorwaarden aan || De wijzigingen gaan in op {day}.",
            "Update van ons privacybeleid || Je hoeft niets te doen.",
        ],
        "pl": [
            "Aktualizujemy regulamin || Zmiany wchodzą w życie {day}.",
            "Zmiany w polityce prywatności || Nie musisz nic robić.",
        ],
    },
    "sale": {
        "pt": [
            "Até {pct}% de desconto: só hoje || Nossa maior liquidação da temporada. Compre já!",
            "Promoção {brand} 🔥 || Economize até {pct}% em {item}. Por tempo limitado.",
        ],
        "it": [
            "Fino al {pct}% di sconto: ultimo giorno || I nostri saldi più grandi della stagione. Acquista ora!",
            "Saldi {brand} 🔥 || Risparmia fino al {pct}% su {item}. Solo per poco.",
        ],
        "nl": [
            "Tot {pct}% korting – alleen vandaag || Onze grootste uitverkoop van het seizoen. Shop nu!",
            "De {brand} sale is begonnen 🔥 || Bespaar tot {pct}% op {item}. Op = op.",
        ],
        "pl": [
            "Do {pct}% taniej – tylko dziś || Największa wyprzedaż sezonu. Kupuj teraz!",
            "Wyprzedaż w {brand} 🔥 || Oszczędź do {pct}% na {item}. Tylko przez chwilę.",
        ],
    },
    "code": {
        "pt": [
            "{pct}% de desconto no próximo pedido || Use o cupom DESC{pct} no checkout. Válido até {day}.",
            "Um presente para você 🎁 || Ganhe {price} de desconto com este cupom exclusivo.",
        ],
        "it": [
            "{pct}% di sconto sul prossimo ordine || Usa il codice sconto SCONTO{pct}. Valido fino al {day}.",
            "Un regalo per te 🎁 || Approfitta di {price} di sconto con questo coupon esclusivo.",
        ],
        "nl": [
            "{pct}% korting op je volgende bestelling || Gebruik kortingscode KORTING{pct}. Geldig tot {day}.",
            "Een cadeautje voor jou 🎁 || Krijg {price} korting met deze exclusieve code.",
        ],
        "pl": [
            "{pct}% zniżki na kolejne zamówienie || Użyj kodu rabatowego RABAT{pct}. Ważny do {day}.",
            "Prezent dla Ciebie 🎁 || Odbierz {price} zniżki z tym kuponem.",
        ],
    },
    "arrivals": {
        "pt": [
            "Novidades acabaram de chegar || Os novos estilos da temporada chegaram. Confira a coleção.",
            "De volta ao estoque: {item} || Aquele que você amou voltou, mas por pouco tempo.",
        ],
        "it": [
            "Novità appena arrivate || I nuovi stili della stagione sono qui. Scopri la collezione.",
            "Di nuovo disponibile: {item} || Quello che ti piaceva è tornato, ma non per molto.",
        ],
        "nl": [
            "Nieuw binnen || De nieuwe stijlen van het seizoen zijn er. Ontdek de collectie.",
            "Weer op voorraad: {item} || Je favoriet is terug, maar niet voor lang.",
        ],
        "pl": [
            "Nowości właśnie dotarły || Nowe style sezonu już są. Odkryj kolekcję.",
            "Znowu dostępne: {item} || Twój ulubiony produkt wrócił, ale nie na długo.",
        ],
    },
    "freeship": {
        "pt": [
            "Frete grátis em tudo neste fim de semana || Sem valor mínimo. Termina {day}.",
            "Seu carrinho está esperando || Você deixou {item} no carrinho. Finalize com frete grátis.",
        ],
        "it": [
            "Spedizione gratuita su tutto nel weekend || Senza minimo d'ordine. Fino a {day}.",
            "Il tuo carrello ti aspetta || Hai lasciato {item} nel carrello. Spedizione gratuita.",
        ],
        "nl": [
            "Gratis verzending op alles dit weekend || Geen minimum. Tot {day}.",
            "Je winkelmandje wacht || Je hebt {item} laten liggen. Rond af met gratis verzending.",
        ],
        "pl": [
            "Darmowa dostawa na wszystko w ten weekend || Bez minimalnej kwoty. Do {day}.",
            "Twój koszyk czeka || Zostawiłeś {item} w koszyku. Dokończ zakupy z darmową dostawą.",
        ],
    },
    "loyalty": {
        "pt": [
            "Você tem {n}00 pontos esperando || Resgate suas recompensas antes de {day}.",
            "Pontos em dobro só esta semana || Ganhe o dobro em cada compra na {brand}.",
        ],
        "it": [
            "Hai {n}00 punti da usare || Riscatta i tuoi premi prima del {day}.",
            "Punti doppi solo questa settimana || Accumula il doppio su ogni acquisto da {brand}.",
        ],
        "nl": [
            "Je hebt {n}00 punten klaarstaan || Wissel je beloningen in voor {day}.",
            "Dubbele punten alleen deze week || Spaar dubbel bij elke aankoop bij {brand}.",
        ],
        "pl": [
            "Masz {n}00 punktów do wykorzystania || Odbierz nagrody przed {day}.",
            "Podwójne punkty tylko w tym tygodniu || Zbieraj podwójnie przy każdym zakupie w {brand}.",
        ],
    },
    "event": {
        "pt": [
            "Participe do nosso webinar gratuito sobre {topic} || Inscreva-se: {day} às 11h. Vagas limitadas.",
            "Você está convidado: {brand} ao vivo || Demonstrações e ofertas especiais. Garanta sua vaga.",
        ],
        "it": [
            "Partecipa al nostro webinar gratuito su {topic} || Iscriviti: {day} alle 11. Posti limitati.",
            "Sei invitato: {brand} Live || Dimostrazioni e offerte speciali. Prenota il tuo posto.",
        ],
        "nl": [
            "Doe mee aan ons gratis webinar over {topic} || Meld je aan: {day} om 11 uur. Beperkt aantal plaatsen.",
            "Je bent uitgenodigd: {brand} Live || Demo's en speciale aanbiedingen. Reserveer je plek.",
        ],
        "pl": [
            "Dołącz do bezpłatnego webinaru o {topic} || Zapisz się: {day} o 11:00. Liczba miejsc ograniczona.",
            "Zaproszenie: {brand} na żywo || Pokazy i oferty specjalne. Zarezerwuj miejsce.",
        ],
    },
    "comeback": {
        "pt": [
            "Sentimos sua falta, {name} || Faz tempo! Aqui está {pct}% de desconto para você voltar.",
            "Recomendado para você || Com base no que você viu: {item} e outras escolhas.",
        ],
        "it": [
            "Ci manchi, {name} || È passato un po'. Ecco il {pct}% di sconto per il tuo ritorno.",
            "Consigliati per te || In base a ciò che hai visto: {item} e altro.",
        ],
        "nl": [
            "We missen je, {name} || Het is even geleden. Hier is {pct}% korting om terug te komen.",
            "Aanbevolen voor jou || Op basis van wat je bekeek: {item} en meer.",
        ],
        "pl": [
            "Tęsknimy, {name} || Dawno Cię nie było. Oto {pct}% zniżki na powrót.",
            "Polecane dla Ciebie || Na podstawie przeglądanych produktów: {item} i więcej.",
        ],
    },
    "comment": {
        "pt": [
            "{name} comentou sua publicação || \"Adorei! Onde é isso?\" Responda no {app}.",
            "{name} respondeu ao seu comentário || Veja a conversa no {app}.",
        ],
        "it": [
            "{name} ha commentato il tuo post || \"Bellissimo! Dov'è?\" Rispondi su {app}.",
            "{name} ha risposto al tuo commento || Guarda la conversazione su {app}.",
        ],
        "nl": [
            "{name} reageerde op je bericht || \"Prachtig! Waar is dit?\" Reageer op {app}.",
            "{name} heeft op je reactie gereageerd || Bekijk het gesprek op {app}.",
        ],
        "pl": [
            "{name} skomentował Twój post || \"Super! Gdzie to jest?\" Odpowiedz na {app}.",
            "{name} odpowiedział na Twój komentarz || Zobacz rozmowę na {app}.",
        ],
    },
    "follow": {
        "pt": [
            "{name} começou a seguir você || Você tem {n} novos seguidores no {app}.",
            "Você tem novos seguidores || {name} e mais {n} pessoas seguem você.",
        ],
        "it": [
            "{name} ha iniziato a seguirti || Hai {n} nuovi follower su {app}.",
            "Hai nuovi follower || {name} e altre {n} persone ti seguono.",
        ],
        "nl": [
            "{name} volgt je nu || Je hebt {n} nieuwe volgers op {app}.",
            "Je hebt nieuwe volgers || {name} en {n} anderen volgen je nu.",
        ],
        "pl": [
            "{name} zaczął Cię obserwować || Masz {n} nowych obserwujących na {app}.",
            "Masz nowych obserwujących || {name} i {n} innych osób obserwuje Cię teraz.",
        ],
    },
    "message": {
        "pt": [
            "{name} enviou uma mensagem para você || \"Oi! Você vem hoje à noite?\" Responda no {app}.",
            "Você tem {n} novas mensagens || {name}: \"Viu isso?\"",
        ],
        "it": [
            "{name} ti ha inviato un messaggio || \"Ciao! Vieni stasera?\" Rispondi su {app}.",
            "Hai {n} nuovi messaggi || {name}: \"Hai visto questo?\"",
        ],
        "nl": [
            "{name} heeft je een bericht gestuurd || \"Hoi! Kom je vanavond?\" Antwoord op {app}.",
            "Je hebt {n} nieuwe berichten || {name}: \"Heb je dit gezien?\"",
        ],
        "pl": [
            "{name} wysłał Ci wiadomość || \"Hej! Przyjdziesz dziś wieczorem?\" Odpowiedz na {app}.",
            "Masz {n} nowe wiadomości || {name}: \"Widziałeś to?\"",
        ],
    },
    "friend": {
        "pt": [
            "{name} enviou um pedido de amizade || Vocês têm {n} amigos em comum.",
            "{name} quer se conectar com você || Aceite o convite de {name} no {app}.",
        ],
        "it": [
            "{name} ti ha inviato una richiesta di amicizia || Avete {n} amici in comune.",
            "{name} vuole collegarsi con te || Accetta l'invito di {name} su {app}.",
        ],
        "nl": [
            "{name} heeft je een vriendschapsverzoek gestuurd || Jullie hebben {n} gemeenschappelijke vrienden.",
            "{name} wil met je connecten || Accepteer de uitnodiging van {name} op {app}.",
        ],
        "pl": [
            "{name} wysłał Ci zaproszenie do znajomych || Macie {n} wspólnych znajomych.",
            "{name} chce nawiązać kontakt || Zaakceptuj zaproszenie od {name} na {app}.",
        ],
    },
    "mention": {
        "pt": [
            "{name} mencionou você em um comentário || Veja o que foi dito no {app}.",
            "{name} marcou você em uma foto || Veja a foto no {app}.",
        ],
        "it": [
            "{name} ti ha menzionato in un commento || Guarda cosa ha scritto su {app}.",
            "{name} ti ha taggato in una foto || Guarda la foto su {app}.",
        ],
        "nl": [
            "{name} noemde je in een reactie || Bekijk wat er gezegd is op {app}.",
            "{name} heeft je getagd in een foto || Bekijk de foto op {app}.",
        ],
        "pl": [
            "{name} wspomniał o Tobie w komentarzu || Zobacz, co napisał na {app}.",
            "{name} oznaczył Cię na zdjęciu || Zobacz zdjęcie na {app}.",
        ],
    },
    "memories": {
        "pt": [
            "Suas lembranças de {n} anos atrás || Relembre este dia com {name}.",
            "Seu perfil foi visto {n} vezes nesta semana || Veja quem visitou seu perfil.",
        ],
        "it": [
            "I tuoi ricordi di {n} anni fa || Rivivi questo giorno con {name}.",
            "Il tuo profilo è stato visto {n} volte questa settimana || Scopri chi ha visitato il tuo profilo.",
        ],
        "nl": [
            "Je herinneringen van {n} jaar geleden || Blik terug op deze dag met {name}.",
            "Je profiel is deze week {n} keer bekeken || Bekijk wie je profiel heeft bezocht.",
        ],
        "pl": [
            "Twoje wspomnienia sprzed {n} lat || Wspomnij ten dzień z {name}.",
            "Twój profil wyświetlono {n} razy w tym tygodniu || Zobacz, kto odwiedził Twój profil.",
        ],
    },
    "like": {
        "pt": [
            "{name} curtiu sua foto || {name} e mais {n} pessoas curtiram sua publicação.",
            "Sua publicação está bombando || {n}0 pessoas reagiram à sua publicação.",
        ],
        "it": [
            "A {name} piace la tua foto || A {name} e ad altre {n} persone piace il tuo post.",
            "Il tuo post sta andando forte || {n}0 persone hanno reagito al tuo post.",
        ],
        "nl": [
            "{name} vindt je foto leuk || {name} en {n} anderen vinden je bericht leuk.",
            "Je bericht valt op || {n}0 mensen reageerden op je bericht.",
        ],
        "pl": [
            "{name} polubił Twoje zdjęcie || {name} i {n} innych osób polubiło Twój post.",
            "Twój post przyciąga uwagę || {n}0 osób zareagowało na Twój post.",
        ],
    },
    "post": {
        "pt": [
            "Re: [{group}] {topic} || Tive o mesmo problema semana passada. Já tentou atualizar para a versão mais recente? Em {day}, {name} escreveu:",
            "[{group}] Dúvida sobre {topic} || Olá a todos, alguém sabe como resolver isso? Obrigado, {name}",
        ],
        "it": [
            "Re: [{group}] {topic} || Ho avuto lo stesso problema la settimana scorsa. Hai provato ad aggiornare? Il {day}, {name} ha scritto:",
            "[{group}] Domanda su {topic} || Ciao a tutti, qualcuno sa come risolvere? Grazie, {name}",
        ],
        "nl": [
            "Re: [{group}] {topic} || Ik had vorige week hetzelfde probleem. Heb je al geüpdatet naar de nieuwste versie? Op {day} schreef {name}:",
            "[{group}] Vraag over {topic} || Hallo allemaal, weet iemand hoe dit op te lossen? Bedankt, {name}",
        ],
        "pl": [
            "Re: [{group}] {topic} || Miałem ten sam problem w zeszłym tygodniu. Próbowałeś zaktualizować? W {day} {name} napisał:",
            "[{group}] Pytanie o {topic} || Cześć wszystkim, czy ktoś wie, jak to rozwiązać? Dzięki, {name}",
        ],
    },
    "digest": {
        "pt": [
            "Resumo de {group} - {n} atualizações em {n} tópicos || {topic} - {n} mensagens. Você recebeu esta mensagem porque está inscrito no grupo.",
            "[{group}] Resumo diário || Tópicos de hoje: {topic}; Ata da reunião.",
        ],
        "it": [
            "Riepilogo di {group} - {n} aggiornamenti in {n} argomenti || {topic} - {n} messaggi. Hai ricevuto questo messaggio perché sei iscritto al gruppo.",
            "[{group}] Riepilogo giornaliero || Argomenti di oggi: {topic}; Verbale.",
        ],
        "nl": [
            "Overzicht van {group} - {n} updates in {n} onderwerpen || {topic} - {n} berichten. Je ontvangt dit bericht omdat je lid bent van de groep.",
            "[{group}] Dagelijks overzicht || Onderwerpen van vandaag: {topic}; Notulen.",
        ],
        "pl": [
            "Podsumowanie {group} - {n} nowych wpisów w {n} wątkach || {topic} - {n} wiadomości. Otrzymujesz tę wiadomość, ponieważ subskrybujesz grupę.",
            "[{group}] Dzienne podsumowanie || Dzisiejsze wątki: {topic}; Protokół.",
        ],
    },
    "topic": {
        "pt": [
            "[{group}] Novo tópico: {topic} || {name} iniciou uma discussão. Responda a este e-mail para participar.",
            "[{group}] {name} respondeu a \"{topic}\" || Responda acima desta linha para publicar no fórum.",
        ],
        "it": [
            "[{group}] Nuova discussione: {topic} || {name} ha aperto una discussione. Rispondi a questa e-mail.",
            "[{group}] {name} ha risposto a \"{topic}\" || Rispondi sopra questa riga per pubblicare nel forum.",
        ],
        "nl": [
            "[{group}] Nieuw onderwerp: {topic} || {name} begon een discussie. Beantwoord deze e-mail om te reageren.",
            "[{group}] {name} reageerde op \"{topic}\" || Antwoord boven deze regel om op het forum te posten.",
        ],
        "pl": [
            "[{group}] Nowy wątek: {topic} || {name} rozpoczął dyskusję. Odpowiedz na tego e-maila.",
            "[{group}] {name} odpowiedział w wątku \"{topic}\" || Odpowiedz nad tą linią, aby opublikować na forum.",
        ],
    },
}
