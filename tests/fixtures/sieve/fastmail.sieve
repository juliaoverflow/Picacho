require ["fileinto", "reject", "vacation", "notify", "envelope", "body", "relational", "regex", "subaddress", "copy", "mailbox", "mboxmetadata", "servermetadata", "date", "index", "comparator-i;ascii-numeric", "variables", "imap4flags", "editheader", "duplicate", "vacation-seconds", "fcc", "special-use"];

### BEGIN GENERATED - DO NOT EDIT
### Spam filtering
if not header :matches "X-Spam-Known-Sender" "yes*" {
  if allof(
    header :contains "X-Backscatter" "yes",
    not header :matches "X-LinkName" "*"
  ) {
    discard;
    stop;
  }
  if header :value "ge" :comparator "i;ascii-numeric" "X-Spam-score" "5" {
    fileinto :specialuse "\\Junk" "INBOX.Junk Mail";
    stop;
  }
}

### Calendar
if allof(header :is "X-ME-Cal-Exists" "exists", header :contains "Content-Type" "text/calendar") {
  set "L1_CAL" "1";
}

### Rules
/* Rule: Receipts */
if header :contains ["From"] ["receipts@shop.example"] {
  fileinto :create "INBOX.Receipts";
  addflag "$label1";
}

if address :domain "From" "example.net" {
  fileinto "INBOX.Partners";
}

if duplicate :seconds 3600 {
  discard;
}

### Vacation
if not exists ["list-id", "list-post", "list-unsubscribe"] {
  vacation :seconds 86400 :from "Ana <ana@fastmail.example>" :subject "Auto: ${1}" :mime text:
Content-Type: multipart/alternative; boundary=foo

--foo
Content-Type: text/plain; charset=utf-8

I'm away.
--foo--
.
;
}
### END GENERATED - DO NOT EDIT

keep;
