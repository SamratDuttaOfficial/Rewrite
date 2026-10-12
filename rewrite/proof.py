"""Proofreading: spelling (red underline) and rule-based grammar (blue underline).

No extra model or word list is needed. A word counts as known when it is in the
WordNet thesaurus, in lemminflect's lexicon (inflected forms), in the small
lists below, or - when the context model is loaded - a whole word in its
vocabulary. Grammar here is a set of careful pattern rules; the engine adds the
context-model checks (agreement, confused words) on top.
"""
import re
from collections import namedtuple

from . import formats
from .textutil import WORD_RE, match_case, sentence_bounds

try:
    import lemminflect as LI
except Exception:
    LI = None

# start/end are offsets into the checked text; fixes may be empty (spelling fixes are looked up lazily)
Issue = namedtuple("Issue", "start end kind message fixes")

TOKEN = re.compile(r"[^\W\d_]+(?:['’-][^\W\d_]+)*")

FUNCTION_WORDS = set("""
a about above across after against all along although am among an and another any anybody anyone anything
anywhere are around as at be because been before behind being below beneath beside besides between beyond both
but by can cannot could dare despite did do does doing done down during each either else elsewhere enough even
every everybody everyone everything everywhere except few fewer for from further had has have having he hence
her here hers herself him himself his how however i if in inside instead into is it its itself just least less
like many may maybe me meanwhile might mine more moreover most much must my myself near need neither never
nevertheless no nobody none nonetheless nor not nothing now nowhere of off often on once one ones oneself only
onto or other others otherwise ought our ours ourselves out outside over own past per perhaps quite rather same
shall she should since so some somebody someone something sometimes somewhere still such than that the their
theirs them themselves then there therefore these they this those though through throughout thus till to too
toward towards under underneath unless unlike until unto up upon us usually versus very via was we were what
whatever when whenever where whereas wherever whether which whichever while whilst who whoever whom whomever
whose why will with within without would yes yet you your yours yourself yourselves
""".split())

CONTRACTIONS = set("""
ain't aren't can't couldn't daren't didn't doesn't don't hadn't hasn't haven't isn't mightn't mustn't needn't
shan't shouldn't wasn't weren't won't wouldn't i'm i've i'll i'd you're you've you'll you'd he's he'll he'd
she's she'll she'd it's it'll it'd we're we've we'll we'd they're they've they'll they'd that's that'll that'd
there's there'll there'd here's what's what'll what're who's who'll who'd who've where's when's why's how's
let's o'clock ma'am y'all could've should've would've might've must've
""".split())

# Everyday and technical words that are missing from WordNet 3.0 and the lexicon.
EXTRA_WORDS = set("""
dataset datasets app apps smartphone smartphones wifi offline online email emails internet website websites blog
blogs pdf pdfs tokenizer tokenizers tokenization tokenize tokenized checkbox checkboxes frontend backend
hyperparameter hyperparameters pretrained pretraining finetune finetuned finetuning embedding embeddings
paraphraser paraphrasers markdown latex docstring docstrings uninstaller installer installers preprocess
preprocessed preprocessing postprocessing runtime runtimes workflow workflows codebase toolbar toolbars dropdown
filename filenames username usernames timestamp timestamps namespace namespaces login logout signup metadata
changelog readme config configs repo repos api apis url urls html css json yaml xml csv sql http https cpu cpus
gpu gpus ram ui ux ios android linux macos unix ubuntu debian github gitlab wikipedia google youtube facebook
twitter linkedin instagram javascript typescript python numpy pytorch tensorflow jupyter docker kubernetes
roberta bert gpt llm llms onnx wordnet async boolean bool int str enum struct tuple tuples iterable iterator
callback callbacks middleware plugin plugins webhook webhooks endpoint endpoints localhost unicode ascii utf
regex regexes substring substrings subclass subclasses superclass refactor refactored refactoring autocomplete
autosave hotkey hotkeys screenshot screenshots unsubscribe upload uploads uploaded uploading download downloads
downloaded downloading downloadable inbox emoji emojis hashtag hashtags podcast podcasts startup startups
blockchain bitcoin cryptocurrency crowdfunding onboarding stakeholder stakeholders roadmap roadmaps deliverable
deliverables scalable scalability usability interoperability maintainability reproducibility explainability
performant benchmark benchmarks benchmarking ablation ablations overfitting underfitting overfit regularization
backpropagation softmax relu logits logit multimodal multilingual monolingual unsupervised preprint preprints
arxiv doi et al etc vs ie eg viz approx misc info intro param params arg args kwargs stdin stdout stderr todo
pre post non anti co multi sub meta pseudo quasi inter intra hyper micro macro nano mega giga kilo milli semi
ms kb mb gb tb khz mhz ghz px pt dpi fps kbps mbps okay ok yeah hmm hello hi bye thanks gonna wanna gotta kinda
mr mrs dr prof jr sr january february march april may june july august september october november december
monday tuesday wednesday thursday friday saturday sunday english french german spanish italian chinese japanese
korean russian arabic hindi bengali indian american british european african asian australian canadian america
england britain france germany spain italy china japan india russia europe africa asia australia canada london
paris berlin tokyo delhi rewrite rewrites rewording reworded rephrase rephrased typo typos programmatically
txt docx odt rtf tex dmg exe png jpg jpeg gif svg zip admin admins diff diffs stats popup popups sync
""".split())

# Frequent misspellings and their corrections. These are never accepted, even when some word list has them.
MISSPELLINGS = dict(x.split(">") for x in """
accomodate>accommodate accomodation>accommodation acheive>achieve acheived>achieved accross>across
agressive>aggressive alot>a_lot aparent>apparent apparantly>apparently appearence>appearance
arguement>argument basicly>basically becuase>because becasue>because beacuse>because becuse>because
begining>beginning beleive>believe belive>believe buisness>business calender>calendar catagory>category
cemetary>cemetery collegue>colleague comming>coming commitee>committee completly>completely
concious>conscious curiousity>curiosity definately>definitely definitly>definitely dissapear>disappear
dissapoint>disappoint embarass>embarrass enviroment>environment existance>existence experiance>experience
familar>familiar finaly>finally foriegn>foreign fourty>forty freind>friend goverment>government gaurd>guard
happend>happened harrass>harass harrassment>harassment hieght>height heigth>height immediatly>immediately
imediately>immediately independant>independent intresting>interesting knowlege>knowledge liason>liaison
libary>library lisence>license maintainance>maintenance mispell>misspell neccessary>necessary
necesary>necessary noticable>noticeable occassion>occasion ocassion>occasion occured>occurred
occuring>occurring occurence>occurrence parliment>parliament persistant>persistent posession>possession
prefered>preferred privelege>privilege priviledge>privilege probaly>probably publically>publicly realy>really
recieve>receive recieved>received recieves>receives recieving>receiving recomend>recommend
reccomend>recommend refered>referred refering>referring relevent>relevant relavant>relevant religous>religious
remeber>remember rember>remember resistence>resistance responsability>responsibility rythm>rhythm
seperate>separate seperately>separately seperated>separated sieze>seize similiar>similar simmilar>similar
sincerly>sincerely succesful>successful sucessful>successful succesfull>successful sucessfully>successfully
supercede>supersede suprise>surprise tendancy>tendency teh>the hte>the thier>their tommorow>tomorrow
tommorrow>tomorrow tounge>tongue truely>truly truley>truly unfortunatly>unfortunately untill>until
wierd>weird wich>which whitch>which writting>writing writen>written adress>address begginer>beginner
bizzare>bizarre carefull>careful usefull>useful helpfull>helpful beautifull>beautiful awfull>awful
greatful>grateful gratefull>grateful guarentee>guarantee garantee>guarantee knowledgable>knowledgeable
lenght>length strenght>strength wieght>weight millenium>millennium miniscule>minuscule
mischievious>mischievous proffesional>professional pronounciation>pronunciation questionaire>questionnaire
refference>reference referance>reference restaraunt>restaurant resturant>restaurant sentance>sentence
speach>speech threshhold>threshold vaccum>vacuum vaccuum>vacuum visable>visible aswell>as_well
infact>in_fact incase>in_case atleast>at_least eachother>each_other everytime>every_time thankyou>thank_you
noone>no_one ofcourse>of_course abit>a_bit alittle>a_little inorder>in_order upto>up_to allways>always
alway>always allready>already allmost>almost amoung>among anual>annual aquire>acquire
aquaintance>acquaintance breif>brief cheif>chief choosen>chosen collumn>column comparision>comparison
compatability>compatibility concensus>consensus consistant>consistent dependance>dependence
dependancy>dependency desparate>desperate developement>development diffrent>different diferent>different
dilema>dilemma effecient>efficient eigth>eighth equiptment>equipment exagerate>exaggerate excede>exceed
exellent>excellent explaination>explanation extreem>extreme facinating>fascinating feild>field
flourescent>fluorescent foward>forward futher>further happyness>happiness heirarchy>hierarchy
hygeine>hygiene inteligent>intelligent intrest>interest irresistable>irresistible labratory>laboratory
neice>niece ninty>ninety nineth>ninth oportunity>opportunity orignal>original paralel>parallel
parrallel>parallel peice>piece percieve>perceive performence>performance permanant>permanent
posible>possible potatos>potatoes tomatos>tomatoes practicle>practical presance>presence
proceedure>procedure programatically>programmatically propoganda>propaganda psycology>psychology
reciept>receipt recogize>recognize rediculous>ridiculous repitition>repetition saftey>safety
scedule>schedule shedule>schedule sence>sense seige>siege sofware>software specifc>specific strech>stretch
stuborn>stubborn sucess>success succes>success sufficent>sufficient suposed>supposed surpress>suppress
temperture>temperature tempature>temperature transfered>transferred twelth>twelfth unecessary>unnecessary
unneccessary>unnecessary usally>usually useing>using vehical>vehicle volenteer>volunteer whereever>wherever
withold>withhold yeild>yield taht>that adn>and nad>and jsut>just waht>what whta>what tihs>this thsi>this
peopel>people poeple>people alos>also aslo>also sicne>since knwo>know konw>know liek>like soem>some
tehn>then thna>than wiht>with wtih>with woudl>would shoudl>should coudl>could doesnt>doesn't dont>don't
didnt>didn't isnt>isn't arent>aren't wasnt>wasn't werent>weren't havent>haven't hasnt>hasn't hadnt>hadn't
wouldnt>wouldn't couldnt>couldn't shouldnt>shouldn't ive>I've youre>you're theyre>they're thats>that's
whats>what's
""".split())
MISSPELLINGS = {k: v.replace("_", " ") for k, v in MISSPELLINGS.items()}

_PREFIXES = ("un", "re", "non", "pre", "over", "under", "mis", "multi", "co", "sub", "inter", "anti", "semi",
             "super", "hyper", "micro", "macro", "meta", "post", "self", "cross", "auto", "pseudo", "de", "dis")
_LETTERS = "abcdefghijklmnopqrstuvwxyz"


def _edits1(w):
    splits = [(w[:i], w[i:]) for i in range(len(w) + 1)]
    out = {a + b[1:] for a, b in splits if b}
    out.update(a + b[1] + b[0] + b[2:] for a, b in splits if len(b) > 1)
    out.update(a + c + b[1:] for a, b in splits if b for c in _LETTERS)
    out.update(a + c + b for a, b in splits for c in _LETTERS)
    out.discard(w)
    return out


class Speller:
    def __init__(self, thes, user_words=()):
        self.T = thes
        self.user = {w.lower() for w in user_words}
        self.session = set()  # "Ignore" lasts until the app closes
        self.vocab = None  # whole-word tokens of the context model, once it is loaded
        self._known, self._fixes = {}, {}
        self._words = None  # every listed word, built on first use to speed up corrections

    # -- word lists -----------------------------------------------------------------
    def set_vocab(self, words):
        self.vocab = set(words) if words else None
        self._known.clear()

    def add(self, word, session=False):
        (self.session if session else self.user).add(word.replace("’", "'").lower())
        self._known.clear()

    def in_dictionary(self, lw):
        """A word from the thesaurus or the lexicon (no abbreviations, no guessing)."""
        return lw in FUNCTION_WORDS or lw in self.T.I or bool(LI and LI.getAllLemmas(lw))

    def _listed(self, lw):
        return lw not in MISSPELLINGS and (lw in EXTRA_WORDS or lw in CONTRACTIONS or self.in_dictionary(lw))

    def _inflected(self, lw):
        """A regular inflection the lexicon lacks (dependencies): its rule-based lemma must be a
        thesaurus word of that part of speech and must inflect back to exactly this spelling."""
        if not LI:
            return False
        for upos, wn in (("NOUN", "n"), ("VERB", "v"), ("ADJ", "a")):
            for lem in LI.getAllLemmasOOV(lw, upos).get(upos, ()):
                if lem != lw and lem in self.T.I and any(self.T.S[s][0] == wn for s in self.T.I[lem]):
                    forms = LI.getAllInflections(lem, upos) or LI.getAllInflectionsOOV(lem, upos)
                    if any(lw in v for v in forms.values()):
                        return True
        return False

    def _word_set(self):
        if self._words is None:
            words = set(FUNCTION_WORDS) | EXTRA_WORDS | CONTRACTIONS | {w for w in self.T.I if " " not in w}
            try:  # lemminflect keeps its lexicon in a private dict; without it corrections are just slower
                words.update(w for w in LI.Lemmatizer()._getLemmaDict() if w.isalpha() and w.islower())
            except Exception:
                pass
            self._words = words - set(MISSPELLINGS)
        return self._words

    def zipf(self, lw):
        """Rough commonness of a known word (0..7)."""
        if lw in FUNCTION_WORDS:
            return 6.0
        z = self.T.zipf(lw)
        if not z and LI:
            z = max((self.T.zipf(x) for v in LI.getAllLemmas(lw).values() for x in v), default=0.0)
            z = z - 0.3 if z else 0.0
        return z or (3.0 if lw in EXTRA_WORDS or lw in CONTRACTIONS else 1.5)

    def known(self, word):
        lw = word.replace("’", "'").lower()
        hit = self._known.get(lw)
        if hit is None:
            hit = self._known[lw] = self._check(lw)
        return hit

    def _check(self, lw):
        if len(lw) <= 2 or lw in self.user or lw in self.session:
            return True
        if lw in MISSPELLINGS:
            return False
        if self._listed(lw) or (self.vocab and (lw in self.vocab or lw.capitalize() in self.vocab)):
            return True
        if "'" in lw:  # possessives
            return any(lw.endswith(sfx) and self.known(lw[:-len(sfx)]) for sfx in ("'s", "s'", "'"))
        if "-" in lw:
            return all(self.known(p) for p in lw.split("-"))
        if self._inflected(lw):
            return True
        for p in _PREFIXES:  # unhelpfully, reranked, nonlinear
            if lw.startswith(p) and len(lw) - len(p) >= 4 and self._listed(lw[len(p):]):
                return True
        return self._compound(lw)

    def _compound(self, lw):
        """checkbox, filename, timestep: two common nouns written as one word."""
        T = self.T
        for i in range(3, len(lw) - 2):
            a, b = lw[:i], lw[i:]
            if b in ("full", "ting", "able") or a in FUNCTION_WORDS or b in FUNCTION_WORDS:
                continue
            if all(x in T.I and T.zipf(x) >= 3.3 and any(T.S[s][0] == "n" for s in T.I[x]) for x in (a, b)):
                return True
        return False

    # -- checking ---------------------------------------------------------------------
    def check(self, text, protected=()):
        """Spans of words that look misspelled: [(start, end)]."""
        out, names, starts = [], set(), []
        for m in TOKEN.finditer(text):
            w, s, e = m.group().replace("’", "'"), m.start(), m.end()
            if len(w) < 3 or not w.isascii() or any(c.isupper() for c in w[1:]):
                continue  # other scripts, ACRONYMS, camelCase
            before, after = text[max(0, s - 2):s], text[e:e + 2]
            if before[-1:] in _ID_BEFORE or before[-1:].isdigit() or after[:1] in _ID_AFTER or after[:1].isdigit():
                continue  # identifiers, paths, x86
            if (before[-1:] == "." and before[:1].isalnum()) or (after[:1] == "." and after[1:2].isalnum()):
                continue  # file.txt, example.com
            if formats.overlaps(protected, s, e):
                continue
            if w[0].isupper():
                if _starts_sentence(text, s):
                    starts.append((s, e, w))
                else:
                    names.add(w)  # capitalised inside a sentence: a name
            elif not self.known(w):
                out.append((s, e))
        for s, e, w in starts:
            # A capitalised first word is only flagged when it clearly is a typo of a common word,
            # so names at the start of a sentence are left alone.
            if w in names or self.known(w):
                continue
            lw = w.lower()
            if lw in MISSPELLINGS or any(self._listed(c) and self.zipf(c) >= 3.3 for c in _edits1(lw)):
                out.append((s, e))
        out.sort()
        return out

    # -- corrections --------------------------------------------------------------------
    def suggest(self, word, n=8):
        """Likely corrections, best first: [(text, prior score)]."""
        lw = word.replace("’", "'").lower()
        if lw not in self._fixes:
            self._fixes[lw] = self._suggest(lw)
            if len(self._fixes) > 2000:
                self._fixes.pop(next(iter(self._fixes)))
        return [(match_case(word, t) if t[:1].islower() else t, sc) for t, sc in self._fixes[lw]][:n]

    def _suggest(self, lw):
        words, scores = self._word_set(), {}

        def offer(cands, dist):
            for cand in cands:
                if len(cand) > 1 and cand != lw:
                    sc = self.zipf(cand) - 2.2 * dist + (0.6 if cand[:1] == lw[:1] else 0.0)
                    scores[cand] = max(sc, scores.get(cand, -99.0))

        e1 = _edits1(lw)
        offer(e1 & words, 1)
        if len(scores) < 4 and len(lw) <= 16:
            for c in e1:
                offer((_edits1(c) & words) - scores.keys(), 2)
        def common(x):
            return x in FUNCTION_WORDS or (len(x) > 2 and x in words and self.zipf(x) >= 4.3)

        for i in range(1, len(lw)):  # a missing space: "alot", "infact"
            a, b = lw[:i], lw[i:]
            if common(a) and common(b):
                scores[a + " " + b] = min(self.zipf(a), self.zipf(b)) - 2.6
        out = sorted(scores.items(), key=lambda x: -x[1])[:12]
        if lw in MISSPELLINGS:
            best = MISSPELLINGS[lw]
            out = [(best, 9.0)] + [x for x in out if x[0] != best]
        return out


_ID_BEFORE, _ID_AFTER = tuple("_\\@#/&"), tuple("_@/")


def _starts_sentence(text, s):
    i = s - 1
    while i >= 0 and text[i] in " \t\"'“‘([*_#>-":
        i -= 1
    return i < 0 or text[i] in ".!?:\n"


# -- grammar rules ------------------------------------------------------------------------

_SILENT_H = re.compile(r"hour|honest|honou?r|heir", re.I)
_YOO = re.compile(r"uni(?![nm]|de)|unanim|use|usu|usa|usi|uti|uto|ute|ura|ure|uri|uro|eu|ew|ubi|uk|ufo|uv|"
                  r"ones?(?![a-z])|oneself|once", re.I)
_AAN_SKIP = {"historic", "historical", "historian", "hotel", "herb", "herbal", "homage", "hilarious", "habitual"}
_AN_LETTERS = set("aefhilmnorsx")  # letter names that start with a vowel sound: an x-ray, an n-gram


def _article_for(word):
    if re.match(r"[A-Za-z]-", word):
        return "an" if word[0].lower() in _AN_LETTERS else "a"
    if _SILENT_H.match(word) or (word[0].lower() in "aeiou" and not _YOO.match(word)):
        return "an"
    return "a"


_MODALS = r"(?:could|should|would|might|must)(?:n['’]t)?"
_NEGAUX = (r"(?:didn't|doesn't|don't|couldn't|wouldn't|shouldn't|can't|cannot|won't|"
           r"did not|does not|do not|could not|would not|should not|will not)")
_DOUBLE_OK = {"that", "had", "is", "her", "very", "so", "no", "ha", "bye", "yeah", "really", "much", "many",
              "far", "more", "long", "blah", "ho", "knock", "now", "there", "well", "oh", "ah"}
_COMPARATIVES = ("more|less|better|worse|greater|fewer|higher|lower|larger|smaller|faster|slower|rather|easier|"
                 "harder|longer|shorter|bigger|cheaper|stronger|weaker")
# a pronoun that opens a clause (so "does it have" and "let it do" are not touched)
_CLAUSE = (r"(?:^|(?<=[.!?;:]\s)|(?<=,\s)|(?<=\band\s)|(?<=\bbut\s)|(?<=\bbecause\s)|(?<=\bso\s)|(?<=\bwhen\s))"
           r"[\"“(]?")
_AGREE = [
    ("he|she|it", {"are": "is", "have": "has", "do": "does", "don't": "doesn't", "aren't": "isn't",
                   "haven't": "hasn't"}),
    ("we|they|you", {"is": "are", "was": "were", "has": "have", "does": "do", "doesn't": "don't",
                     "isn't": "aren't", "wasn't": "weren't", "hasn't": "haven't"}),
    ("I", {"is": "am", "are": "am", "has": "have", "does": "do", "doesn't": "don't"}),
]
_I = "The pronoun “I” is always capitalised."
# (pattern, group to underline, replacement or function of the match, message)
_RULES = [
    (r"\b(" + _MODALS + r")\s+(of)\b(?!\s+course)", 2, "have", "After “{1}”, use “have”, not “of”."),
    (r"\b(?:" + _COMPARATIVES + r")\s+(then)\b", 1, "than", "Use “than” for comparisons; “then” is about time."),
    (r"\b(its)\s+(?:an?|the|not|been|also)\b(?!\s+(?:priori|posteriori))", 1, "it's",
     "“it's” (it is / it has) is needed here; “its” shows possession."),
    (r"\b(it['’]s)\s+own\b", 1, "its", "Possessive “its” has no apostrophe."),
    (r"\b(their)\s+(?:is|are|was|were)\b", 1, "there", "Use “there” here; “their” shows possession."),
    (r"\b(there)\s+own\b", 1, "their", "Use the possessive “their” here."),
    (r"\b(your)\s+welcome\b", 1, "you're", "“you're” (you are) is needed here."),
    (r"\b(?:am|is|are|was|were|be|been)\s+(?:not\s+)?(suppose)\s+to\b", 1, "supposed", "The phrase is “supposed to”."),
    (r"(?-i:(?<![\w.'’-])(i)(?=\s+(?:am|was|have|had|would|think|know|believe|hope|don['’]t)\b))", 1, "I", _I),
    (r"(?-i:(?<![\w.'’-])(i)(?=['’](?:m|ve|ll|d)\b))", 1, "I", _I),
    (r"\b(?:more|most)\s+(better|worse|easier|harder|faster|simpler|best|worst|easiest)\b", 0,
     lambda m: m.group(1), "“{1}” is already a comparative; drop the word before it."),
    (r"(?<=\w)( +([,;!?]))(?=\s|$)", 1, lambda m: m.group(2), "No space before this punctuation mark."),
    (r"(?<=[A-Za-z])( +(\.))(?=\s|$)", 1, lambda m: m.group(2), "No space before the full stop."),
    (r"(?-i:(?<=[a-z])(,)(?=[A-Za-z]{2}))", 1, ", ", "Add a space after the comma."),
    (r"(?<=\w)(,,|;;)", 1, lambda m: m.group(1)[0], "Doubled punctuation."),
]
_RULES = [(re.compile(rx, re.I | re.M), g, rep, msg) for rx, g, rep, msg in _RULES]
_DOUBLED = re.compile(r"(?<![\w'’-])([A-Za-z]{2,})[ \t]+\1(?![\w'’-])", re.I)
_ARTICLE = re.compile(r"\b(an?)[ \t]+([A-Za-z][A-Za-z-]*)", re.I)
_NEG_VERB = re.compile(r"\b(" + _NEGAUX + r")\s+([a-z]+)\b", re.I)
_HAVE_VERB = re.compile(r"\b(?:have|has|had|having)\s+([a-z]+)\b", re.I)
_LABEL = re.compile(r"(?:\b[A-Z][a-z]+|[(\[])\s*$")  # "Appendix A is ...": a letter, not an article


def grammar_rules(text, protected=(), speller=None):
    """High-precision pattern rules: [Issue]. These need no model."""
    out = []

    def add(s, e, msg, fix):
        if e > s and fix != text[s:e] and not formats.overlaps(protected, s, e):
            out.append(Issue(s, e, "gram", msg, [fix]))

    for rx, grp, rep, msg in _RULES:
        for m in rx.finditer(text):
            old = m.group(grp)
            new = rep(m) if callable(rep) else rep
            if old[:1].isupper() and new[:1].islower():
                new = match_case(old, new)
            add(*m.span(grp), msg.replace("{1}", m.group(1)), new)

    for m in _DOUBLED.finditer(text):
        if m.group(1).lower() not in _DOUBLE_OK:
            add(m.start(), m.end(), f"“{m.group(1)}” is repeated.", m.group(1))

    for m in _ARTICLE.finditer(text):
        art, nxt = m.group(1), m.group(2)
        low = nxt.lower()
        if len(nxt) < 2 or any(c.isupper() for c in nxt[1:]) or not re.search("[aeiouy]", low) \
                or low in _AAN_SKIP or low in FUNCTION_WORDS or (art.isupper() and len(art) == 2):
            continue  # acronyms, "values a and b", html
        if art == "A" and _LABEL.search(text, max(0, m.start() - 20), m.start()):
            continue
        want = _article_for(nxt)
        if art.lower() != want and not formats.overlaps(protected, m.start(2), m.end(2)):
            add(m.start(1), m.end(1), f"Use “{want}” before “{nxt}”.", match_case(art, want))

    for subj, table in _AGREE:
        rx = re.compile(_CLAUSE + r"(" + subj + r")\s+(" + "|".join(map(re.escape, table)) + r")\b(?!['’])",
                        re.M if subj == "I" else re.M | re.I)
        for m in rx.finditer(text):
            fix = table[m.group(2).lower()]
            add(m.start(2), m.end(2), f"“{m.group(1)}” takes “{fix}”, not “{m.group(2)}”.",
                match_case(m.group(2), fix))

    T = speller.T if speller else None
    if T and LI:
        for m in _NEG_VERB.finditer(text):  # didn't went -> didn't go
            w = m.group(2).lower()
            tags = T.tags(w)
            if tags and tags <= {"VBD", "VBN", "VBZ", "NNS"} and tags & {"VBD", "VBN", "VBZ"}:
                lem = (LI.getAllLemmas(w).get("VERB") or [w])[0]
                add(m.start(2), m.end(2), f"Use the base form “{lem}” after “{m.group(1)}”.", lem)
        for m in _HAVE_VERB.finditer(text):  # have went -> have gone
            w = m.group(1).lower()
            if w not in ("was", "were", "did") and T.tags(w) == {"VBD"}:
                lem = (LI.getAllLemmas(w).get("VERB") or [None])[0]
                part = LI.getInflection(lem, "VBN") if lem else ()
                if part:
                    add(m.start(1), m.end(1), f"After “have”, use the past participle “{part[0]}”.", part[0])

    if speller:
        for s, e in sentence_bounds(text):  # "... the end. the next one ..."
            m = WORD_RE.match(text, s)
            if not m or not m.group().islower() or len(m.group()) < 2:
                continue
            prev = re.search(r"([a-z]{4,})\.[ \t]+$", text[max(0, s - 40):s])
            if prev and speller.in_dictionary(prev.group(1)) and speller.in_dictionary(m.group()):
                add(m.start(), m.end(), "A sentence starts with a capital letter.", m.group().capitalize())

    return dedupe(out)


def dedupe(issues):
    """Sort issues and drop any that overlap an earlier one."""
    out = []
    for it in sorted(issues, key=lambda x: (x.start, x.end)):
        if not out or it.start >= out[-1].end:
            out.append(it)
    return out
