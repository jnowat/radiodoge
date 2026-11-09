# usage: python resolve-version.py NEWVER   (run in worktree mid-merge or after merge)
import sys, re, subprocess
new = sys.argv[1]
V = r'0\.\d+\.\d+'

def git(*a):
    return subprocess.run(['git', *a], capture_output=True, text=True, encoding='utf-8').stdout

VERFILES = [
    'crates/radiodoge-cli/Cargo.toml',
    'crates/radiodoge-core/Cargo.toml',
    'radiodoge-gui/src-tauri/Cargo.toml',
    'radiodoge-gui/src-tauri/tauri.conf.json',
    'radiodoge-gui/package.json',
    'radiodoge-gui/package-lock.json',
    'Cargo.lock',
    'README.md',
    'ROADMAP.md',
    'docs/USER_MANUAL.md',
]
conf = [f for f in git('diff', '--name-only', '--diff-filter=U').split('\n') if f]

def rd(p):
    return open(p, encoding='utf-8', newline='').read()

def wr(p, s):
    open(p, 'w', encoding='utf-8', newline='').write(s)

HK = re.compile(r'<<<<<<< [^\n]*\n(.*?)=======\r?\n(.*?)>>>>>>> [^\n]*\n', re.S)
bad = []
for f in conf:
    if f == 'CHANGELOG.md':
        continue
    s = rd(f)

    def fix(m):
        o, t = m.group(1), m.group(2)
        if re.sub(V, 'V', o) == re.sub(V, 'V', t):
            return t
        bad.append(f)
        return m.group(0)

    s = HK.sub(fix, s)
    wr(f, s)
if bad:
    print('UNRESOLVED', sorted(set(bad)))
    sys.exit(2)

# CHANGELOG: base version sections + this branch's one new section (renamed to NEW).
# Keep [Unreleased] text from HEAD so intentional Unreleased edits are not wiped
# when finish.ps1 checks out the base CHANGELOG before calling this script.
def secs(s):
    parts = re.split(r'(?m)^(?=## )', s)
    return parts[0], parts[1:]

base = git('show', 'origin/0.0.1-Beta-1:CHANGELOG.md')
ours = git('show', 'HEAD:CHANGELOG.md')
norm = lambda x: re.sub(r'\s+', ' ', x.split('\n', 1)[1] if '\n' in x else '').strip()
bh, bs = secs(base)
_, os_ = secs(ours)
nl = '\r\n' if '\r\n' in base else '\n'
bs = list(bs)
our_unrel = next((x for x in os_ if x.startswith('## [Unreleased]')), None)
if our_unrel:
    our_unrel = our_unrel.replace('\r\n', '\n').replace('\n', nl)
    if not our_unrel.endswith(nl):
        our_unrel += nl
    for i, x in enumerate(bs):
        if x.startswith('## [Unreleased]'):
            if norm(x) != norm(our_unrel):
                print('preserving Unreleased from HEAD')
            bs[i] = our_unrel
            break
bodies = {norm(x) for x in bs}
uniq = [x for x in os_ if not x.startswith('## [Unreleased]') and norm(x) not in bodies]
print('branch-only changelog sections:', [u.split('\n')[0] for u in uniq])
if len(uniq) != 1:
    print('EXPECTED 1')
    sys.exit(3)
u = re.sub(r'^## \[?' + V + r'\]?', f'## [{new}]', uniq[0], count=1)
idx = next(i for i, x in enumerate(bs) if not x.startswith('## [Unreleased]'))
u = u.replace('\r\n', '\n').replace('\n', nl)
if not u.endswith(nl + nl):
    u = u.rstrip() + nl + nl
wr('CHANGELOG.md', bh + ''.join(bs[:idx]) + u + ''.join(bs[idx:]))

def sub(p, pat, rep, n=1):
    s = rd(p)
    s2, k = re.subn(pat, rep, s, count=n, flags=re.M)
    assert k >= 1, (p, pat)
    wr(p, s2)

for p in VERFILES[:3]:
    sub(p, r'^version = "' + V + '"', f'version = "{new}"')
for p in VERFILES[3:5]:
    sub(p, r'"version": "' + V + '"', f'"version": "{new}"')
sub(VERFILES[5], r'"version": "' + V + '"', f'"version": "{new}"', 2)
for c in ['radiodoge-cli', 'radiodoge-core', 'radiodoge-gui']:
    sub('Cargo.lock', r'(name = "' + c + r'"\r?\nversion = ")' + V + '"', r'\g<1>' + new + '"')
sub('README.md', r'\(v' + V + r'\)', f'(v{new})')
sub('ROADMAP.md', r'\*\*v' + V + r'\*\* \(app\)', f'**v{new}** (app)')
sub('docs/USER_MANUAL.md', r'\*App v' + V, f'*App v{new}')
sub('docs/USER_MANUAL.md', r'`RadioDoge v' + V + '`', f'`RadioDoge v{new}`')
left = git('grep', '-l', '-E', '^(<<<<<<<|>>>>>>>) ')
if left.strip():
    print('MARKERS LEFT', left)
    sys.exit(4)
print('resolved ->', new)
