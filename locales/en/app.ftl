# Picacho's interface in English: the source of every translation.
#
# Each file holds the text of the view it's named after (mobile.ftl: the
# phone layout). A message's id names where it shows. Variables are written
# { $name }; numeric ones can choose a plural form:
#   { $count ->
#       [one] { $count } message
#      *[other] { $count } messages
#   }
# Variables ending in _shown (or called shown) are a count already written
# out with its digit grouping; select on the plain count beside them.
# Keep a leading "@" in tooltip text: it marks where Picacho adds the
# action's keyboard shortcut. {"…"} quotes spaces Fluent would otherwise trim,
# and braces. See src/i18n/mod.rs.

app-name = Picacho

# Two sentences one after the other, as in a setting's description and its
# second sentence (Chinese and Japanese, for example, put no space between)
app-sentences = { $first } { $second }

## Settings → General

settings-language = Language
settings-language-automatic = Automatic

# The language of what Picacho writes into mail others receive
settings-outgoing-language = Outgoing mail

settings-outgoing-language-caption = For reply headers, invitations and receipts.

# Outgoing mail in the same language as Picacho's
settings-outgoing-language-same = Same as Picacho
