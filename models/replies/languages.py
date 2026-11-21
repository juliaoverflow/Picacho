"""The mail the reply model is trained on, in every language it suggests
replies in.

Each language (phrases_a.py to phrases_d.py) has, for every situation in
SITUATIONS, a few ways a message might say it ("Are you free on {day}
afternoon?"), and greetings, small talk, sign-offs and subjects to wrap them
in. {day} is a weekday in that language (the date model's, models/tbs),
{time} a clock time and {name} a first name. Alternatives are split by "|";
data.py trains on even-numbered alternatives and holds the odd ones out
(English has four of each, the others two), so the held-out half measures
wording the model never saw.

Nothing here is real mail; it is written to look like it.
"""

import importlib.util
from pathlib import Path

from phrases_a import L as A
from phrases_b import L as B
from phrases_c import L as C
from phrases_d import L as D

HERE = Path(__file__).parent

L = {**A, **B, **C, **D}

# What each situation calls for: the replies in catalog.json that fit,
# the likeliest first. "automated" (newsletters, notifications) calls for
# none.
SITUATIONS = {
    "thanks": ["youre_welcome", "happy_to_help", "no_problem"],
    "availability": ["check_calendar", "yes_sure", "cant_make_it", "what_time"],
    "meeting": ["works_for_me", "see_you_then", "check_calendar", "another_time"],
    "invite": ["love_to", "count_me_in", "cant_make_it", "sounds_good"],
    "request": ["will_do", "on_it", "yes_sure", "take_care_of_it"],
    "review": ["take_a_look", "will_do", "looks_good"],
    "attachment": ["thanks_for_sending", "got_it_thanks", "received", "take_a_look"],
    "fyi": ["thanks_for_letting_me_know", "got_it_thanks", "understood"],
    "good_news": ["great_news", "congratulations", "happy_for_you"],
    "bad_news": ["sorry_to_hear", "here_to_help", "take_care"],
    "sick": ["feel_better", "take_care", "no_worries"],
    "late": ["no_problem", "no_worries", "see_you_soon", "thanks_for_letting_me_know"],
    "apology": ["no_worries", "no_problem", "understood"],
    "yes_no": ["yes_sure", "sorry_no", "think_about_it", "sounds_good"],
    "opinion": ["sounds_good", "good_idea", "lets_discuss", "think_about_it"],
    "confirm": ["confirmed", "received", "yes_sure"],
    "call": ["call_soon", "good_time", "yes_sure"],
    "urgent": ["on_it", "done_today", "understood", "will_do"],
    "checkin": ["doing_well", "good_to_hear", "catch_up"],
    "birthday": ["thank_you_so_much", "thanks", "appreciate_it"],
    "holiday": ["same_to_you", "thank_you_so_much", "thanks"],
    "offer_help": ["thanks", "appreciate_it", "thank_you_so_much"],
    "choice": ["either_works", "check_calendar", "let_me_check"],
    "invoice": ["received", "take_care_of_it", "thanks"],
    "intro": ["nice_to_meet", "thanks_intro", "looking_forward"],
    "interview": ["looking_forward", "thank_you_so_much", "check_calendar"],
    "praise": ["thank_you_so_much", "glad_you_liked", "appreciate_it"],
    "cancel": ["no_problem", "understood", "reschedule"],
    "move": ["works_for_me", "no_problem", "check_calendar"],
    "followup": ["sorry_delay", "get_back_soon", "thanks_reminder"],
    "question": ["let_me_check", "get_back_soon"],
    "done": ["perfect", "great_news", "thanks"],
    "met": ["likewise", "thanks", "looking_forward"],
    "automated": [],
}
WRAPPERS = ["greet", "filler", "sign", "subject"]

LANGUAGES = list(L)
# How often each language comes up, beside the rest.
WEIGHT = {"en": 5, "es": 2.5, "de": 2.5, "fr": 2.5, "pt": 2.5, "it": 2, "ja": 2, "zh": 2, "ru": 2, "nl": 2}

NAMES = ["Alex", "Sam", "Jordan", "Taylor", "Morgan", "Chris", "Robin", "Ana", "Luis", "Mei", "Ravi",
         "Olga", "Kofi", "Yuki", "Fatima", "Noor", "Elif", "Lars", "Ingrid", "Marta", "Tomás", "Priya"]
TIMES = ["9:00", "9:30", "10:00", "11:00", "13:00", "14:30", "15:00", "16:00", "17:30"]

ENGLISH_DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]


def _date_model_languages():
    spec = importlib.util.spec_from_file_location("tbs_languages", HERE.parent / "tbs" / "languages.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.L


_TBS = _date_model_languages()


def days(lang):
    """The weekdays as they are written in `lang`."""
    if lang == "en":
        return ENGLISH_DAYS
    return [d.split("|")[0] for d in _TBS[lang]["days"]]


def alternatives(lang, key):
    return L[lang][key].split("|")


def check():
    """Every language has every situation and wrapper, and every reply the
    situations name is in the catalog."""
    import json
    catalog = json.loads((HERE / "catalog.json").read_text(encoding="utf-8"))
    intents = set(catalog["intents"])
    for sit, fits in SITUATIONS.items():
        missing = [i for i in fits if i not in intents]
        assert not missing, (sit, missing)
    for lang, entries in L.items():
        assert lang in catalog["replies"], lang
        for key in list(SITUATIONS) + WRAPPERS:
            assert key in entries and entries[key].strip(), (lang, key)
        extra = set(entries) - set(SITUATIONS) - set(WRAPPERS)
        assert not extra, (lang, extra)
        days(lang)
    assert set(L) == set(catalog["replies"]), set(catalog["replies"]) ^ set(L)


if __name__ == "__main__":
    check()
    print(f"{len(L)} languages, {len(SITUATIONS)} situations: all present")
