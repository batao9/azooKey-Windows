#!/usr/bin/env python3
"""Summarize the opt-in Swift Jev evaluation report; no API calls or credentials."""
import json
import math
import statistics
import sys
from collections import Counter


def summarize(records):
    baseline = sum(r['baseline'] in r['acceptable'] for r in records)
    return {
        'cases': len(records), 'kkc_correct': baseline,
        'jev_correct': sum(r['accepted'] for r in records),
        'oracle': sum(r['oracle'] for r in records),
        'improved': sum(r['accepted'] and r['baseline'] not in r['acceptable'] for r in records),
        'regressed': sum(not r['accepted'] and r['baseline'] in r['acceptable'] for r in records),
        'fallbacks': sum(r['fallback'] for r in records),
        'actual_candidates_median': statistics.median(r['actual_count'] for r in records),
        'jev_ms_median': statistics.median(r['jev_ms'] for r in records),
        'jev_ms_p95': sorted(r['jev_ms'] for r in records)[math.ceil(len(records) * .95) - 1],
        'kkc_ms_median': statistics.median(r['kkc_ms'] for r in records),
    }


def main(path):
    with open(path, encoding='utf-8') as source:
        records = json.load(source)['records']
    result = {}
    for n in sorted({r['n'] for r in records}):
        rows = [r for r in records if r['n'] == n]
        result[str(n)] = {
            'all': summarize(rows),
            'without_proper_names': summarize([r for r in rows if r['category'] != 'public_proper_name']),
            'categories': {
                category: summarize([r for r in rows if r['category'] == category])
                for category in sorted({r['category'] for r in rows})
            },
        }
    actual_calls = [r for r in records if not r['reused'] and r['actual_count'] > 1]
    result['actual_calls'] = len(actual_calls)
    result['call_failures'] = dict(Counter(r.get('error_category') for r in actual_calls if r['fallback']))
    result['actual_call_ms_median'] = statistics.median(r['jev_ms'] for r in actual_calls) if actual_calls else None
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main(sys.argv[1])
