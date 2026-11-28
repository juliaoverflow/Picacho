//! Mail merge: one message per row of a spreadsheet (CSV), each with that
//! row's values in the draft's placeholders (`{first_name}`, `{company}`…,
//! any column by its heading) and sent to the row's address.
use crate::manage::composing::{Placeholders, fill_placeholders_with};
use crate::manage::model::Draft;
use anyhow::{Result, bail};

/// A spreadsheet read from CSV: its headings and rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sheet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// The largest spreadsheet a merge takes.
pub const MAX_ROWS: usize = 2000;

/// Reads CSV (RFC 4180): fields separated by commas, semicolons or tabs
/// (whichever the first line uses most), quoted with `"` where they hold
/// one of those, a quote (doubled) or a line break. The first row names
/// the columns. UTF-8, with or without its byte order mark; otherwise
/// Windows-1252, as spreadsheets save it.
pub fn parse_csv(bytes: &[u8]) -> Result<Sheet> {
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t.trim_start_matches('\u{feff}').to_owned(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned(),
    };
    let first = text.lines().next().unwrap_or("");
    let delimiter = [',', ';', '\t'].into_iter().max_by_key(|d| first.matches(*d).count()).unwrap_or(',');
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.trim().is_empty() => {
                field.clear();
                quoted = true;
            }
            c if c == delimiter => record.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            c => field.push(c),
        }
        if records.len() > MAX_ROWS + 1 {
            bail!(t!("manage-merge-too-many-rows", max = MAX_ROWS));
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    let mut records = records.into_iter();
    let Some(columns) = records.next() else {
        bail!(t!("manage-merge-the-file-is-empty"));
    };
    let columns: Vec<String> = columns.into_iter().map(|c| c.trim().to_owned()).collect();
    let rows: Vec<Vec<String>> = records
        .map(|mut r| {
            r.resize(columns.len(), String::new());
            r.into_iter().map(|f| f.trim().to_owned()).collect()
        })
        .collect();
    if rows.is_empty() {
        bail!(t!("manage-merge-no-rows-below-the-headings"));
    }
    Ok(Sheet { columns, rows })
}

/// A column's placeholder name: its heading in lowercase, spaces and
/// dashes as underscores (`First Name` is `{first_name}`).
pub fn key(column: &str) -> String {
    column.trim().to_lowercase().replace([' ', '-'], "_")
}

impl Sheet {
    /// The column with addresses: one headed like "email", else the first
    /// whose values look like addresses.
    pub fn email_column(&self) -> Option<usize> {
        let named = self.columns.iter().position(|c| {
            let k = key(c);
            matches!(k.as_str(), "email" | "e_mail" | "email_address" | "e_mail_address" | "mail" | "address" | "to")
        });
        named.or_else(|| {
            (0..self.columns.len()).find(|&i| {
                let looks = self.rows.iter().filter(|r| r.get(i).is_some_and(|v| is_address(v))).count();
                looks * 2 > self.rows.len()
            })
        })
    }
    fn column(&self, names: &[&str]) -> Option<usize> {
        self.columns.iter().position(|c| names.contains(&key(c).as_str()))
    }
    /// Row `i`'s values by placeholder name.
    pub fn values(&self, i: usize) -> Vec<(String, String)> {
        let row = &self.rows[i];
        self.columns.iter().zip(row).map(|(c, v)| (key(c), v.clone())).collect()
    }
    /// The name row `i` gives its recipient: a "name" column, else its
    /// first and last names.
    pub fn name(&self, i: usize) -> String {
        let row = &self.rows[i];
        let get = |c: Option<usize>| c.and_then(|c| row.get(c)).map(|v| v.trim().to_owned()).unwrap_or_default();
        let name = get(self.column(&["name", "full_name", "fullname", "display_name"]));
        if !name.is_empty() {
            return name;
        }
        let first = get(self.column(&["first_name", "firstname", "first", "given_name"]));
        let last = get(self.column(&["last_name", "lastname", "last", "surname", "family_name"]));
        format!("{first} {last}").trim().to_owned()
    }
}

/// Whether `v` looks like one email address.
pub fn is_address(v: &str) -> bool {
    let v = v.trim();
    let Some((local, domain)) = v.rsplit_once('@') else { return false };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && !v.contains(char::is_whitespace) && !v.contains([',', ';', '<', '>'])
}

/// `draft` for row `i` of `sheet`: to the row's address (named as the row
/// names its recipient), its subject, text and HTML with the row's values
/// in their placeholders (and `base`'s, the sender's and today's, for the
/// rest). A new draft of its own, without Cc or Bcc. `None` for a row
/// without an address.
pub fn personalize(draft: &Draft, base: &Placeholders, sheet: &Sheet, email_column: usize, i: usize) -> Option<Draft> {
    let email = sheet.rows.get(i)?.get(email_column)?.trim().to_owned();
    if !is_address(&email) {
        return None;
    }
    let name = sheet.name(i);
    let values = sheet.values(i);
    let p = Placeholders { name: name.clone(), email: email.clone(), ..base.clone() };
    let subject = fill_placeholders_with(&draft.subject, &Placeholders { subject: draft.subject.clone(), ..p.clone() }, &values, false);
    let p = Placeholders { subject: subject.clone(), ..p };
    let to = if name.is_empty() {
        email
    } else if name.contains([',', '"', '<', '>', ';', '@']) {
        format!("\"{}\" <{email}>", name.replace(['"', '\\'], ""))
    } else {
        format!("{name} <{email}>")
    };
    Some(Draft {
        id: uuid::Uuid::new_v4().to_string(),
        to,
        cc: String::new(),
        bcc: String::new(),
        subject,
        body: fill_placeholders_with(&draft.body, &p, &values, false),
        html: fill_placeholders_with(&draft.html, &p, &values, true),
        remote: None,
        uploaded: String::new(),
        send_at: None,
        send_error: String::new(),
        ..draft.clone()
    })
}

/// A mail merge being sent: what's left, kept in the database so it goes
/// on where it stopped when Picacho starts again.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Queue {
    /// The draft it was made from, which goes once all are sent.
    pub draft: String,
    pub account: String,
    pub total: usize,
    /// The messages not sent yet, in order.
    pub remaining: Vec<Draft>,
    /// Those that couldn't be: to whom, and why.
    pub failed: Vec<String>,
}

const QUEUE: &str = "mail_merge_queue";

impl Queue {
    pub fn load(store: &crate::manage::store::Store) -> Option<Queue> {
        store.preference(QUEUE).filter(|j| !j.is_empty()).and_then(|j| serde_json::from_str(&j).ok())
    }
    pub fn save(&self, store: &crate::manage::store::Store) -> Result<()> {
        store.set_preference(QUEUE, &serde_json::to_string(self)?)
    }
    pub fn clear(store: &crate::manage::store::Store) -> Result<()> {
        store.remove_preference(QUEUE)
    }
    /// Takes message `id` off what's left, as kept, before it is sent: a
    /// message is never sent twice, though one being sent as Picacho stops
    /// is lost (and named, as failed, on the next start's notice).
    pub fn take(store: &crate::manage::store::Store, id: &str) -> Result<()> {
        if let Some(mut q) = Queue::load(store) {
            if let Some(i) = q.remaining.iter().position(|d| d.id == id) {
                let d = q.remaining.remove(i);
                // Taken, until it is sent: a stop in between says so.
                q.failed.push(format!("{} ({})", d.to, t!("manage-merge-interrupted")));
            }
            q.save(store)?;
        }
        Ok(())
    }
    /// Message `id` went (`error` None) or couldn't.
    pub fn done(store: &crate::manage::store::Store, to: &str, error: Option<String>) -> Result<()> {
        if let Some(mut q) = Queue::load(store) {
            let interrupted = format!("{to} ({})", t!("manage-merge-interrupted"));
            q.failed.retain(|f| *f != interrupted);
            if let Some(e) = error {
                q.failed.push(format!("{to} ({e})"));
            }
            q.save(store)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_is_read_with_quotes_delimiters_and_encodings() {
        let s = parse_csv(b"\xef\xbb\xbfEmail,First Name,Note\r\nana@example.org,Ana,\"Hi, \"\"you\"\"\nline two\"\r\n\r\nbo@example.org,Bo\n").unwrap();
        assert_eq!(s.columns, ["Email", "First Name", "Note"]);
        assert_eq!(s.rows, [vec!["ana@example.org", "Ana", "Hi, \"you\"\nline two"], vec!["bo@example.org", "Bo", ""]]);
        let s = parse_csv(b"name;mail\nZo\xe9;zoe@example.org").unwrap();
        assert_eq!(s.rows[0], ["Zoé", "zoe@example.org"]);
        assert_eq!(s.email_column(), Some(1));
        let s = parse_csv(b"who\tcontact\nAna\tana@example.org\n").unwrap();
        assert_eq!(s.email_column(), Some(1), "found by its values");
        assert!(parse_csv(b"").is_err());
        assert!(parse_csv(b"just,headings\n").is_err());
    }

    #[test]
    fn each_row_gets_its_own_message() {
        let sheet = parse_csv(b"First Name,Last Name,Email,Company\nAna,Lopez,ana@example.org,Acme & Co\nBo,,not-an-address,X\n").unwrap();
        let draft = Draft {
            id: "d1".into(),
            to: "ignored@example.org".into(),
            cc: "boss@example.org".into(),
            subject: "Hello {first_name}".into(),
            body: "Dear {name}, about {Company}. {unknown}".into(),
            html: "<p>Dear {first_name} at {company}</p>".into(),
            formatted: true,
            ..Draft::default()
        };
        let base = Placeholders { my_name: "Sam".into(), ..Placeholders::default() };
        let column = sheet.email_column().unwrap();
        let d = personalize(&draft, &base, &sheet, column, 0).unwrap();
        assert_eq!(d.to, "Ana Lopez <ana@example.org>");
        assert!(d.cc.is_empty());
        assert_ne!(d.id, "d1");
        assert_eq!(d.subject, "Hello Ana");
        assert_eq!(d.body, "Dear Ana Lopez, about Acme & Co. {unknown}");
        assert_eq!(d.html, "<p>Dear Ana at Acme &amp; Co</p>");
        assert!(personalize(&draft, &base, &sheet, column, 1).is_none());
        assert!(is_address("a.b@c.io") && !is_address("a b@c.io") && !is_address("a@b"));
    }
}
