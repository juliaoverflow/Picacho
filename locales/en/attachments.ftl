# src/display/attachments.rs: the attachment browser.

attachments-attachments = Attachments

attachments-browse-attachments = Browse Attachments

attachments-search = Search names, senders and contents

attachments-all-accounts = All Accounts

attachments-every-mailbox = Every mailbox

attachments-with-person = With { $person }

attachments-all-kinds = All

attachments-kind-images = Images

attachments-kind-pdfs = PDFs

attachments-kind-documents = Documents

attachments-kind-spreadsheets = Spreadsheets

attachments-kind-presentations = Presentations

attachments-kind-archives = Archives

attachments-kind-audio = Audio

attachments-kind-video = Video

attachments-kind-calendar = Invitations & cards

attachments-kind-emails = Emails

attachments-kind-other = Other

attachments-newest-first = Newest first

attachments-oldest-first = Oldest first

attachments-largest-first = Largest first

attachments-by-name = By name

attachments-loading = Loading…

attachments-loading-preview = Loading preview…

attachments-count = { $count ->
        [one] { $shown } attachment · { $size }
       *[other] { $shown } attachments · { $size }
    }

attachments-unlisted = { $count ->
        [one] { $shown } more message has attachments that are listed once it's downloaded.
       *[other] { $shown } more messages have attachments that are listed once they're downloaded.
    }

attachments-none = No attachments here

attachments-none-match = No attachments match

attachments-pick = Select

attachments-pick-all = Select All

attachments-pick-none = Clear Selection

attachments-picked = { $count ->
        [one] { $shown } selected
       *[other] { $shown } selected
    }

attachments-save-picked = Save to Folder…

attachments-saving-short = Saving…

attachments-save-here = Save Here

attachments-saving = { $count ->
        [one] Saving { $shown } attachment…
       *[other] Saving { $shown } attachments…
    }

attachments-saved = { $count ->
        [one] Saved { $shown } attachment
       *[other] Saved { $shown } attachments
    }

attachments-saved-some = { $count ->
        [one] Saved { $saved } of { $shown } attachment. Couldn't save { $name }: { $e }
       *[other] Saved { $saved } of { $shown } attachments. Couldn't save { $name }: { $e }
    }

attachments-show-message = Show Message

attachments-preview = Preview

attachments-close-preview = @Close preview

attachments-no-preview = No preview for this kind of file
