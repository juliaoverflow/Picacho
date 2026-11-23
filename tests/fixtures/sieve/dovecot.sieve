require ["fileinto", "mailbox", "variables", "envelope", "subaddress", "enotify", "editheader", "spamtest", "relational", "comparator-i;ascii-numeric", "mime", "foreverypart", "vacation", "vacation-seconds", "include", "fcc"];

# Move spam
if spamtest :value "ge" :comparator "i;ascii-numeric" "6" {
    fileinto :create "Junk";
    stop;
}

# Sort by plus address
if envelope :detail :matches "to" "*" {
    set :lower "folder" "${1}";
    if string :is "${folder}" "" {
        keep;
    } else {
        fileinto :create "Lists/${folder}";
    }
}

# Tag and notify
if header :contains "subject" "[ALERT]" {
    addheader :last "X-Alerted" "yes";
    deleteheader :index 1 "X-Old";
    notify :importance "1" :message "Alert: ${subject}" "mailto:pager@example.com";
}

# Strip executables
foreverypart {
    if header :mime :param "filename" :matches "Content-Disposition" "*.exe" {
        fileinto "Quarantine";
        break;
    }
}

include :global "after";

vacation :seconds 3600 :subject "Away" :from "me@example.com" :fcc "Sent" "Back Monday.";
