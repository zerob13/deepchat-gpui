#!/usr/bin/env python3
"""Dependency-free validator for the frozen parity contracts."""
import json, subprocess, sys
from pathlib import Path

ROOT=Path(__file__).resolve().parents[2]
REF=ROOT/'parity/reference-baseline.json'; MAN=ROOT/'parity/manifest.json'
VALID={'identified','specified','implemented','verified','blocked','not_applicable','waived'}
KINDS={'source','test','release-notes','workflow','configuration'}
PLATFORMS={'macos-arm64':'aarch64-apple-darwin','macos-x64':'x86_64-apple-darwin','windows-arm64':'aarch64-pc-windows-msvc','windows-x64':'x86_64-pc-windows-msvc','linux-arm64':'aarch64-unknown-linux-gnu','linux-x64':'x86_64-unknown-linux-gnu'}

def fail(msg): print(f'ERROR: {msg}'); return False
def main():
    ok=True
    try: baseline=json.loads(REF.read_text()); manifest=json.loads(MAN.read_text())
    except Exception as e: return int(fail(f'invalid JSON: {e}') is False)
    if baseline['reference']['commit'] != manifest['referenceCommit']: ok=fail('reference commits differ') and ok
    if set(baseline['discovery']['statusVocabulary']) != VALID: ok=fail('baseline status vocabulary mismatch') and ok
    if set(manifest['statusValues']) != VALID: ok=fail('manifest status vocabulary mismatch') and ok
    ids=[f.get('id') for f in manifest.get('features',[])]
    if len(ids)!=len(set(ids)): ok=fail('feature IDs are not unique') and ok
    known=set(ids)
    for f in manifest['features']:
        s=f.get('status')
        if s not in VALID: ok=fail(f"{f.get('id')}: invalid status {s}") and ok
        for dep in f.get('dependencies',[]):
            if dep not in known: ok=fail(f"{f['id']}: unknown dependency {dep}") and ok
        platforms=f.get('platforms',[])
        if platforms != ['all'] and set(platforms)-set(PLATFORMS): ok=fail(f"{f['id']}: invalid platform") and ok
        ps=f.get('platformStatus',{})
        if platforms == ['all'] and ps: ok=fail(f"{f['id']}: platforms=all requires empty platformStatus") and ok
        if platforms != ['all'] and set(ps) != set(platforms): ok=fail(f"{f['id']}: platformStatus incomplete") and ok
        for ev in f.get('referenceEvidence',[]):
            if not isinstance(ev,dict) or set(ev)-{'kind','path','selector'} or not {'kind','path'}<=set(ev) or ev['kind'] not in KINDS or not isinstance(ev['path'],str): ok=fail(f"{f['id']}: malformed evidence") and ok; continue
            cmd=['git','-C',baseline['reference']['repository'],'cat-file','-e',baseline['reference']['commit']+':'+ev['path']]
            if subprocess.run(cmd,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode: ok=fail(f"{f['id']}: missing frozen path {ev['path']}") and ok
        if s in {'verified','waived','blocked'} and not f.get('gaps') and s!='waived': ok=fail(f"{f['id']}: {s} requires gaps/reason") and ok
        if s=='verified' and not f.get('verification'): ok=fail(f"{f['id']}: verified requires verification") and ok
    print('parity contract: PASS' if ok else 'parity contract: FAIL')
    return 0 if ok else 1
if __name__=='__main__': sys.exit(main())
