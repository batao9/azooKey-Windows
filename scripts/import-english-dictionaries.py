#!/usr/bin/env python3
"""Offline import of pinned English wordlists and UniDic. Not run by the IME."""
import argparse
import csv
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import urllib.request
import zipfile

ESDB_REV = '1e5b7d3a72f47a71da5d28686c1dd4b397178485'
CSPELL_REV = 'ae9c8d4dc15024377cc64b21d77ce863a35e0c9d'
UNIDIC_SHA = '6cce98269214ce7de6159f61a25ffc5b436375c098cc86d6aa98c0605cbf90d4'
CSPELL_FILES = ['software-tools.txt', 'software-services.txt', 'network-protocols.txt']
ROOT = Path(__file__).resolve().parent.parent


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--esdb', required=True, type=Path)
    parser.add_argument('--unidic', required=True, type=Path)
    args = parser.parse_args()
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=args.esdb, text=True).strip()
    if revision != ESDB_REV or sha(args.unidic.read_bytes()) != UNIDIC_SHA:
        raise ValueError('Source revision/hash mismatch')
    # Keep original case filtering before normalization: no proper-name inventory.
    raw = subprocess.check_output([
        './scowl', 'word-list', '50', 'A', '1', '--categories=', '--wo-poses=abbr',
    ], cwd=args.esdb, text=True)
    english = {word: 'esdb50' for word in raw.splitlines() if re.fullmatch('[a-z]{4,20}', word)}
    source_hashes = {}
    base = f'https://raw.githubusercontent.com/streetsidesoftware/cspell-dicts/{CSPELL_REV}/dictionaries/software-terms/'
    for name in CSPELL_FILES:
        data = urllib.request.urlopen(base + 'src/' + name, timeout=30).read()
        source_hashes[name] = sha(data)
        for line in data.decode('utf-8').splitlines():
            word = line.split('#', 1)[0].strip()
            if re.fullmatch('[A-Za-z]{4,20}', word):
                english.setdefault(word.lower(), 'cspell')
    # These lists include some alphabet test strings; not English words.
    for word in list(english):
        if word in 'abcdefghijklmnopqrstuvwxyz' or len(set(word)) == 1:
            del english[word]
    output = ROOT / 'data' / 'english-dictionaries'
    output.mkdir(parents=True, exist_ok=True)
    notices = ['English dictionary source notices\n',
               'ESDB: https://github.com/en-wl/wordlist/tree/' + ESDB_REV + '\n',
               (args.esdb / 'Copyright').read_text(),
               '\nCSpell software-terms: ' + base + '\n',
               urllib.request.urlopen(base + 'LICENSE', timeout=30).read().decode('utf-8')]
    pairs = set()
    char_ids = set((ROOT / 'server-swift/azooKey_dictionary_storage/Dictionary/louds/charID.chid').read_text())
    with zipfile.ZipFile(args.unidic) as archive:
        prefix = 'unidic-mecab-2.1.2_src/'
        notices += ['\nUniDic 2.1.2: https://clrd.ninjal.ac.jp/unidic/\nBSD license selected.\n',
                    archive.read(prefix + 'BSD').decode('utf-8')]
        with archive.open(prefix + 'lex.csv') as binary:
            for row in csv.reader(io.TextIOWrapper(binary, encoding='utf-8')):
                reading, lemma, goshu = row[0], row[11], row[16]
                _, sep, spelling = lemma.partition('-')
                spelling = re.sub(r'（[^（）]*）$', '', spelling)
                if (sep and goshu == '外' and re.fullmatch('[ァ-ヺー]{2,20}', reading)
                        and set(reading) <= char_ids
                        and spelling in english and re.fullmatch('[a-z]{4,20}', spelling)):
                    pairs.add((reading, spelling))
    # Explicitly exclude known semantic traps; this is orthographic assistance,
    # not unrestricted loanword etymology or Japanese-to-English translation.
    pairs = {(r, w) for r, w in pairs if r not in {'コンセント', 'マンション', 'アルバイト', 'カルテ'}}
    # Independently authored modern-technical supplement; source is explicit.
    supplements = {('キャッシュ', 'cache'), ('リベース', 'rebase'), ('デプロイ', 'deploy'),
                   ('バックエンド', 'backend'), ('フロントエンド', 'frontend')}
    english.update({word: english.get(word, 'project') for _, word in supplements})
    pairs.update(supplements)
    kana_text = '# reading\tword\tsource\n' + ''.join(
        f'{r}\t{w}\t{"project" if (r, w) in supplements else "unidic"}\n' for r, w in sorted(pairs))
    english_text = '# spelling\tsource\n' + ''.join(f'{w}\t{s}\n' for w, s in sorted(english.items()))
    (output / 'katakana.tsv').write_text(kana_text, encoding='utf-8')
    (output / 'spelling.tsv').write_text(english_text, encoding='utf-8')
    notice_text = '\n'.join(line.rstrip() for line in '\n'.join(notices).splitlines())
    (output / 'NOTICE.txt').write_text(notice_text.rstrip() + '\n', encoding='utf-8')
    manifest = {'esdb_revision': ESDB_REV, 'esdb_command': './scowl word-list 50 A 1 --categories= --wo-poses=abbr',
                'cspell_revision': CSPELL_REV, 'cspell_files_sha256': source_hashes,
                'unidic_version': '2.1.2', 'unidic_sha256': UNIDIC_SHA,
                'katakana_pairs': len(pairs), 'spellings': len(english),
                'katakana_sha256': sha(kana_text.encode()), 'spelling_sha256': sha(english_text.encode())}
    (output / 'sources.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    # Runtime binary resources are generated offline from these TSV inputs by
    # generate-english-dictionary.mjs, never embedded into executable source.
    print(json.dumps(manifest, indent=2))


if __name__ == '__main__':
    main()
