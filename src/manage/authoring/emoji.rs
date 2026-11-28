//! The composer's emoji picker: the emoji it offers, by category, with
//! words to find them by (English, as Unicode names them).

/// A group of emoji, as the picker's tabs show them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Smileys,
    People,
    Nature,
    Food,
    Activities,
    Travel,
    Objects,
    Symbols,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::Smileys,
        Category::People,
        Category::Nature,
        Category::Food,
        Category::Activities,
        Category::Travel,
        Category::Objects,
        Category::Symbols,
    ];
    /// The emoji its tab shows.
    pub fn icon(self) -> &'static str {
        match self {
            Category::Smileys => "😀",
            Category::People => "👋",
            Category::Nature => "🌿",
            Category::Food => "🍎",
            Category::Activities => "⚽",
            Category::Travel => "✈️",
            Category::Objects => "💡",
            Category::Symbols => "❤️",
        }
    }
}

/// An emoji and the words that find it.
#[derive(Clone, Copy, Debug)]
pub struct Emoji {
    pub text: &'static str,
    pub words: &'static str,
    pub category: Category,
}

macro_rules! emoji {
    ($cat:ident: $($text:literal $words:literal),* $(,)?) => {
        [$(Emoji { text: $text, words: $words, category: Category::$cat }),*]
    };
}

const SMILEYS: [Emoji; 48] = emoji!(Smileys:
    "😀" "grinning face smile happy", "😃" "smiley happy joy", "😄" "smile happy laugh", "😁" "grin beaming",
    "😆" "laughing satisfied", "😅" "sweat smile relief", "🤣" "rofl rolling floor laughing", "😂" "joy tears laughing",
    "🙂" "slightly smiling", "🙃" "upside down silly", "😉" "wink", "😊" "blush smiling eyes",
    "😇" "innocent halo angel", "🥰" "smiling hearts love", "😍" "heart eyes love", "🤩" "star struck excited",
    "😘" "kiss blowing", "😋" "yum delicious tongue", "😛" "tongue out", "😜" "wink tongue crazy",
    "🤪" "zany crazy", "🤗" "hug hugging", "🤭" "hand over mouth oops", "🤫" "shush quiet secret",
    "🤔" "thinking hmm", "🤐" "zipper mouth", "😐" "neutral", "😑" "expressionless",
    "😶" "no mouth speechless", "😏" "smirk", "😒" "unamused", "🙄" "eye roll",
    "😬" "grimace awkward", "😌" "relieved", "😔" "pensive sad", "😴" "sleeping tired zzz",
    "😷" "mask sick", "🤒" "thermometer ill sick", "🥳" "party celebration", "😎" "cool sunglasses",
    "🤓" "nerd glasses", "😕" "confused", "😟" "worried", "😮" "surprised open mouth wow",
    "😢" "cry sad tear", "😭" "sob crying", "😱" "scream fear shocked", "😡" "angry pouting mad",
);

const PEOPLE: [Emoji; 32] = emoji!(People:
    "👋" "wave hello hi bye", "🤚" "raised back hand", "✋" "raised hand stop high five", "👌" "ok okay perfect",
    "🤌" "pinched fingers", "✌️" "victory peace", "🤞" "crossed fingers luck hope", "🤟" "love you gesture",
    "🤘" "rock on horns", "🤙" "call me shaka", "👈" "point left", "👉" "point right",
    "👆" "point up", "👇" "point down", "☝️" "index up", "👍" "thumbs up yes like approve +1",
    "👎" "thumbs down no dislike -1", "✊" "fist raised", "👊" "punch fist bump", "👏" "clap applause bravo",
    "🙌" "raising hands hooray celebrate", "👐" "open hands", "🤲" "palms up", "🤝" "handshake deal agreement",
    "🙏" "pray please thanks folded hands", "✍️" "writing hand", "💪" "muscle strong flex", "🧠" "brain smart",
    "👀" "eyes look see", "👤" "person silhouette", "👥" "people team group", "🙋" "raising hand question volunteer",
);

const NATURE: [Emoji; 28] = emoji!(Nature:
    "🐶" "dog puppy", "🐱" "cat kitten", "🐭" "mouse", "🐰" "rabbit bunny",
    "🦊" "fox", "🐻" "bear", "🐼" "panda", "🐨" "koala",
    "🐯" "tiger", "🦁" "lion", "🐮" "cow", "🐷" "pig",
    "🐸" "frog", "🐵" "monkey", "🐔" "chicken", "🐧" "penguin",
    "🐦" "bird", "🦋" "butterfly", "🐝" "bee honeybee", "🐢" "turtle slow",
    "🌵" "cactus desert", "🌲" "evergreen tree", "🌳" "tree", "🌿" "herb leaf plant",
    "🍀" "four leaf clover luck", "🌸" "cherry blossom flower spring", "🌻" "sunflower", "🌹" "rose flower",
);

const FOOD: [Emoji; 28] = emoji!(Food:
    "🍎" "apple red fruit", "🍊" "orange tangerine", "🍋" "lemon", "🍌" "banana",
    "🍉" "watermelon", "🍇" "grapes", "🍓" "strawberry", "🍒" "cherries",
    "🍑" "peach", "🥑" "avocado", "🌶️" "hot pepper chili spicy", "🌽" "corn",
    "🥐" "croissant", "🍞" "bread", "🧀" "cheese", "🍳" "cooking egg breakfast",
    "🍔" "burger hamburger", "🍟" "fries", "🍕" "pizza", "🌮" "taco",
    "🌯" "burrito", "🍣" "sushi", "🍪" "cookie", "🎂" "birthday cake",
    "🍰" "cake shortcake", "☕" "coffee hot beverage tea", "🍺" "beer", "🍷" "wine glass",
);

const ACTIVITIES: [Emoji; 24] = emoji!(Activities:
    "⚽" "soccer football", "🏀" "basketball", "🏈" "american football", "⚾" "baseball",
    "🎾" "tennis", "🏐" "volleyball", "🏓" "ping pong table tennis", "⛳" "golf",
    "🏆" "trophy winner award", "🥇" "gold medal first", "🎉" "party popper tada celebration congratulations", "🎊" "confetti ball",
    "🎈" "balloon party", "🎁" "gift present wrapped", "🎄" "christmas tree", "🎃" "jack o lantern halloween pumpkin",
    "🎮" "video game controller", "🎲" "dice game", "🎯" "target bullseye direct hit", "🎨" "art palette paint",
    "🎵" "music note", "🎸" "guitar", "🎬" "clapper movie film", "📸" "camera flash photo",
);

const TRAVEL: [Emoji; 24] = emoji!(Travel:
    "🚗" "car automobile", "🚕" "taxi", "🚌" "bus", "🚲" "bicycle bike",
    "🚆" "train", "✈️" "airplane plane flight travel", "🚀" "rocket launch ship it", "🛳️" "ship cruise",
    "⛵" "sailboat", "🏠" "house home", "🏢" "office building", "🏥" "hospital",
    "🏫" "school", "🏖️" "beach umbrella vacation", "⛰️" "mountain", "🗺️" "world map",
    "🌍" "globe earth world", "🌙" "moon crescent night", "⭐" "star", "☀️" "sun sunny",
    "⛅" "partly cloudy", "🌧️" "rain cloud", "❄️" "snowflake cold winter", "🌈" "rainbow",
);

const OBJECTS: [Emoji; 32] = emoji!(Objects:
    "💡" "light bulb idea", "📱" "mobile phone", "💻" "laptop computer", "⌨️" "keyboard",
    "🖨️" "printer", "📷" "camera", "📞" "telephone receiver call", "⏰" "alarm clock",
    "⌛" "hourglass time", "📅" "calendar date", "📆" "tear off calendar", "📌" "pushpin pin",
    "📎" "paperclip attachment", "✂️" "scissors cut", "📝" "memo note write", "✏️" "pencil",
    "📚" "books", "📖" "open book read", "📦" "package box parcel", "📫" "mailbox mail",
    "✉️" "envelope email letter", "📧" "e-mail email", "📤" "outbox sent", "📥" "inbox received",
    "🔒" "lock locked secure", "🔑" "key", "🔧" "wrench tool fix", "🔨" "hammer",
    "💰" "money bag", "💳" "credit card payment", "🛒" "shopping cart", "📈" "chart increasing growth up",
);

const SYMBOLS: [Emoji; 32] = emoji!(Symbols:
    "❤️" "red heart love", "🧡" "orange heart", "💛" "yellow heart", "💚" "green heart",
    "💙" "blue heart", "💜" "purple heart", "🖤" "black heart", "💔" "broken heart",
    "💯" "hundred points perfect", "✅" "check mark done yes", "☑️" "check box ballot", "✔️" "heavy check mark",
    "❌" "cross mark no wrong", "❗" "exclamation mark important", "❓" "question mark", "⚠️" "warning caution",
    "🚫" "prohibited no entry forbidden", "⛔" "no entry stop", "🔥" "fire hot lit", "✨" "sparkles new shiny",
    "⚡" "lightning high voltage zap", "💥" "collision boom", "💬" "speech balloon chat comment", "💭" "thought balloon",
    "➡️" "right arrow", "⬅️" "left arrow", "⬆️" "up arrow", "⬇️" "down arrow",
    "🆕" "new button", "🆗" "ok button", "🔴" "red circle", "🟢" "green circle",
);

/// Every emoji the picker offers, in order.
pub fn all() -> impl Iterator<Item = &'static Emoji> {
    SMILEYS
        .iter()
        .chain(PEOPLE.iter())
        .chain(NATURE.iter())
        .chain(FOOD.iter())
        .chain(ACTIVITIES.iter())
        .chain(TRAVEL.iter())
        .chain(OBJECTS.iter())
        .chain(SYMBOLS.iter())
}

/// The emoji in `category`.
pub fn in_category(category: Category) -> Vec<&'static Emoji> {
    all().filter(|e| e.category == category).collect()
}

/// Emoji whose words match `query`, best first.
pub fn search(query: &str) -> Vec<&'static Emoji> {
    let mut found: Vec<(u8, usize, &'static Emoji)> = all()
        .enumerate()
        .filter_map(|(i, e)| super::rank(query, e.words.split_whitespace().next().unwrap_or(""), e.words).map(|r| (r, i, e)))
        .collect();
    found.sort_by_key(|(r, i, _)| (*r, *i));
    found.into_iter().map(|(_, _, e)| e).collect()
}

#[cfg(test)]
#[test]
fn emoji_are_found_by_their_words() {
    assert!(Category::ALL.iter().all(|c| !in_category(*c).is_empty()));
    let found = search("thumb");
    assert_eq!(found[0].text, "👍");
    assert!(search("heart").iter().any(|e| e.text == "❤️"));
    assert!(search("tada").iter().any(|e| e.text == "🎉"));
    assert!(search("qqqq").is_empty());
    // No emoji twice.
    let mut seen = std::collections::HashSet::new();
    assert!(all().all(|e| seen.insert(e.text)), "an emoji is listed twice");
}
