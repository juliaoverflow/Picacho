require ["fileinto","vacation","imap4flags","copy","body","date","relational"];
# rule:[Bills]
if anyof (header :contains "subject" "invoice", address :is "from" "billing@shop.example")
{
	fileinto "INBOX.Bills";
	stop;
}
# rule:[Newsletters]
if allof (header :contains "list-id" "news", not header :contains "subject" "urgent")
{
	addflag "\\Seen";
	fileinto "Newsletters";
}
# rule:[Forward boss]
if address :is "from" "boss@example.com"
{
	redirect :copy "me@mobile.example";
}
# rule:[Large]
if size :over 10M
{
	fileinto "Large";
}
# rule:[Away]
if allof (currentdate :value "ge" "date" "2026-12-20", currentdate :value "le" "date" "2027-01-05")
{
	vacation :days 1 :subject "Out of office" :addresses ["me@example.com","me@example.org"] :from "me@example.com" text:
I am away until January 5th.

..For urgent matters, call the office.
.
;
}
