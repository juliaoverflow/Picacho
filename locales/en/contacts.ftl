# src/display/contacts.rs and nearby views.

contacts-contacts = Contacts

contacts-in-your-address-book-the-composer = { $count ->
        [0] No addresses saved.
        [one] { $count } address saved.
       *[other] { $count } addresses saved.
    }

contacts-contacts-server-carddav = Contacts server (CardDAV)

contacts-signed-in-as-read-read-again = { $how ->
        [account] { $user }, through your { $email } account · Read { $synced }
       *[password] { $user } · Read { $synced }
    }

contacts-reading = Reading…

contacts-read-now = Read Now

contacts-disconnect = Disconnect

contacts-new-address-book = New address book

contacts-make-address-book = Make Address Book

contacts-read-and-edit-your-address-books = Fastmail, iCloud, Nextcloud, Radicale or another CardDAV server.

contacts-server = Server

contacts-user-name = User name

contacts-password = Password

contacts-connecting = Connecting…

contacts-connect = Connect

contacts-files = Files

contacts-vcard-files = vCard files

contacts-import = Import…

contacts-export = Export…

contacts-remove-those-on-this-computer = Remove Those on This Computer

contacts-read-contacts = { $n ->
        [one] Read { $n } contact
       *[other] Read { $n } contacts
    }

contacts-import-2 = Import

contacts-imported-contacts = { $n ->
        [one] Imported { $n } contact
       *[other] Imported { $n } contacts
    }

contacts-contacts-vcf = Contacts.vcf

contacts-exported-contacts = { $n ->
        [one] Exported { $n } contact
       *[other] Exported { $n } contacts
    }

contacts-read-contacts-from = { $n ->
        [one] Read { $n } contact from { $email }
       *[other] Read { $n } contacts from { $email }
    }

contacts-the-server-has-no-one-else = The server has no one else by that.

contacts-found-1-more-on-the-server = Found 1 more on the server

contacts-found-more-on-the-server = Found { $n } more on the server

contacts-write-the-new-name-in-the = Write the new name in the field first.

contacts-renamed-the-address-book = Renamed the address book { $name }

contacts-deleted-the-address-book = Deleted the address book

contacts-made-the-address-book = Made the address book { $name }

contacts-saved = Saved { $name }

contacts-address-books = Address books

contacts-rename-or-delete-the-servers-address = Rename or delete the server's address books.

contacts-show = Show

contacts-new-contacts-go-here = { $name } (new contacts go here)

contacts-rename = Rename

contacts-delete = Delete

contacts-the-server-keeps-mb-of-yours = { $used } MB used, { $left } MB left.

contacts-rename-uses-the-name-written-in = Rename uses the name in the field above.

contacts-new-contact = New contact

contacts-edit-contact = Edit contact

contacts-new-contact-2 = New Contact

contacts-edit-contact-2 = Edit Contact

contacts-name = Name

contacts-email-addresses-separated-by-commas = Email addresses (separated by commas)

contacts-phone-numbers = Phone numbers

contacts-organization = Organization

contacts-note = Note

contacts-postal-addresses = Postal addresses

contacts-birthday = Birthday

contacts-job-title = Job title

contacts-nicknames = Nicknames

contacts-web-and-chat = Web and chat

contacts-anniversary = Anniversary

contacts-members-makes-it-a-group = Members (makes it a group)

contacts-saved-to-your-contacts-server = Saved to your contacts server.

contacts-saved-on-this-computer = Saved on this computer.

contacts-save = Save

contacts-cancel = Cancel

contacts-no-contacts-yet = No contacts yet.

contacts-no-one-matches = No one matches.

contacts-search-the-server = Search the server

contacts-people = People

contacts-showing-200-of-type-to-narrow = Showing 200 of { $total }. Type to narrow them down.

# When the server hasn't been read yet ("Read never")
contacts-never = never
