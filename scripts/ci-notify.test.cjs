'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const {ALWAYS, FULL, ANALYZE, classify, decide, notify, marker} = require('./ci-notify.cjs');

const sha = 'a'.repeat(40);
function fixture(tier = 'full') {
  const runs = [1, 2].map(id => ({id, name: id === 1 ? 'CI' : 'CodeQL',
    status: 'completed', conclusion: 'success', head_sha: sha,
    html_url: `https://github.com/o/r/actions/runs/${id}`}));
  const shards = tier === 'gui' ? ['all'] : ['recovery', 'dat', 'gui-v2', 'rest'];
  const names = [[...ALWAYS, ...FULL, ...shards.map(s => `Formatting and relevant tests (${s})`)], ANALYZE];
  const jobs = names.flatMap((group, i) => group.map((name, k) => ({name, run_id: i + 1,
    status: 'completed', conclusion: tier !== 'full' && FULL.includes(name) ? 'skipped' : 'success',
    check_run_url: `https://api.github.com/repos/o/r/check-runs/${i * 100 + k}`,
    html_url: `https://github.com/o/r/actions/runs/${i + 1}/job/${k}`})));
  const checks = jobs.map(j => ({...j, id: Number(j.check_run_url.split('/').pop())}));
  checks.push({id: 999, name: 'CodeQL', app: {slug: 'github-advanced-security'},
    status: 'completed', conclusion: 'success'});
  return {runs, jobs, checks, statuses: [], tier, mergeable: 'MERGEABLE', mergeStateStatus: 'CLEAN', draft: false};
}

for (const tier of ['full', 'core', 'gui']) {
  test(`${tier}: all applicable gates must pass`, () => assert.equal(decide(fixture(tier)).status, 'READY'));
}
test('classifies paths with the trusted existing policy, including shell metacharacters', () => {
  assert.equal(classify([{filename: 'crates/archivefs-gui/src/view.rs'}]), 'gui');
  assert.equal(classify([{filename: 'crates/archivefs-core/src/catalog.rs'}]), 'core');
  assert.equal(classify([{filename: '$(touch /tmp/DO_NOT_CREATE)'}]), 'full');
});
for (const location of ['runs', 'jobs', 'checks']) {
  test(`${location}: pending takes precedence over failures`, () => {
    const x = fixture();
    x.jobs[1].conclusion = 'failure';
    x[location][0].status = 'in_progress';
    assert.equal(decide(x).status, 'PENDING');
  });
}
test('missing workflow and analysis result stay pending', () => {
  const x = fixture(); x.runs.pop(); assert.equal(decide(x).status, 'PENDING');
  const y = fixture(); y.checks.pop(); assert.equal(decide(y).status, 'PENDING');
});
test('a failed CodeQL run without an analysis-result check still reports FAILED', () => {
  const x = fixture(); x.checks.pop(); x.runs[1].conclusion = 'failure';
  assert.equal(decide(x).status, 'FAILED');
});
for (const conclusion of ['failure', 'cancelled', 'timed_out', 'skipped', 'neutral', 'action_required', null]) {
  test(`mandatory job ${conclusion} fails with its job link`, () => {
    const x = fixture(); x.jobs[0].conclusion = conclusion;
    const result = decide(x);
    assert.equal(result.status, 'FAILED');
    assert.ok(result.problems.some(p => p.url === x.jobs[0].html_url));
  });
}
test('missing mandatory job fails even if workflow says success', () => {
  const x = fixture(); x.jobs.shift(); assert.equal(decide(x).status, 'FAILED');
});
test('a duplicate name from another workflow cannot grant a skip exception', () => {
  const x = fixture('core');
  x.checks.push({id: 1000, name: FULL[0], status: 'completed', conclusion: 'skipped'});
  assert.equal(decide(x).status, 'FAILED');
});
test('full tier cannot skip release/reproducibility; failed classification grants no exceptions', () => {
  const x = fixture(); x.jobs.find(j => j.name === FULL[0]).conclusion = 'skipped';
  assert.equal(decide(x).status, 'FAILED');
  const y = fixture('core'); y.jobs[0].conclusion = 'failure';
  assert.ok(decide(y).problems.some(p => p.name.includes(FULL[0])));
});
test('additional checks and commit statuses cannot be ignored', () => {
  for (const state of ['pending', 'failure', 'error']) {
    const x = fixture(); x.statuses = [{context: 'external', state, target_url: 'https://github.com/o/r'}];
    assert.equal(decide(x).status, state === 'pending' ? 'PENDING' : 'FAILED');
  }
  const x = fixture(); x.checks.at(-1).conclusion = 'failure';
  assert.equal(decide(x).status, 'FAILED');
});
test('conflicts, unknown, draft, behind and blocked are never READY', () => {
  for (const mergeStateStatus of ['UNKNOWN', 'DIRTY', 'BEHIND', 'BLOCKED', 'UNSTABLE']) {
    assert.equal(decide({...fixture(), mergeStateStatus}).status, 'BLOCKED');
  }
  assert.equal(decide({...fixture(), draft: true}).status, 'BLOCKED');
  assert.equal(decide({...fixture(), mergeable: 'UNKNOWN'}).status, 'BLOCKED');
});

function harness({duplicate = false, stale = false, staleBase = false, foreign = false, pending = false} = {}) {
  const x = fixture();
  if (pending) x.runs[1].status = 'in_progress';
  const pr = {number: 7, head: {sha: foreign ? 'b'.repeat(40) : sha},
    base: {sha: 'c'.repeat(40)}, state: 'open', html_url: 'https://github.com/o/r/pull/7'};
  x.runs[0].event = 'pull_request'; x.runs[1].event = 'dynamic';
  const posted = []; const requested = [];
  const rest = {
    pulls: {list: 'prs', listFiles: 'files', get: async () => ({data:
      {...pr, head: {sha: stale ? 'b'.repeat(40) : sha},
        base: {sha: staleBase ? 'd'.repeat(40) : pr.base.sha}}})},
    issues: {listComments: 'comments', createComment: async c => posted.push(c)},
    actions: {listRepoWorkflows: 'workflows', listWorkflowRuns: 'runs', listJobsForWorkflowRun: 'jobs'},
    checks: {listForRef: 'checks'}, repos: {listCommitStatusesForRef: 'statuses'},
  };
  const github = {rest, paginate: async (method, args) => {
    requested.push({method, args});
    return ({prs: [pr], files: [{filename: 'README.md'}],
      comments: duplicate ? [{user: {login: 'github-actions[bot]'}, body: marker(7, sha)}] : [],
      workflows: [{id: 1, path: '.github/workflows/ci.yml'},
        {id: 2, path: 'dynamic/github-code-scanning/codeql'}],
      runs: x.runs.filter(r => r.id === args.workflow_id),
      jobs: x.jobs.filter(j => j.run_id === args.run_id), checks: x.checks, statuses: []})[method];
  }, graphql: async () => ({repository: {pullRequest: {headRefOid: sha, baseRefOid: pr.base.sha,
    state: 'OPEN', isDraft: false, mergeable: 'MERGEABLE', mergeStateStatus: 'CLEAN'}}})};
  return {github, context: {repo: {owner: 'o', repo: 'r'}, payload: {workflow_run: {head_sha: sha}}},
    core: {info() {}}, posted, requested};
}
test('posts exact PR/head, final status, workflow links and owner mention', async () => {
  const h = harness(); await notify(h);
  assert.equal(h.posted.length, 1);
  assert.match(h.posted[0].body, /@kiehntre \*\*READY\*\*.*PR #7/);
  assert.ok(h.posted[0].body.includes(sha));
  assert.match(h.posted[0].body, /actions\/runs\/2/);
  assert.ok(h.requested.filter(r => r.method === 'runs').every(r => r.args.head_sha === sha));
});
for (const mode of ['duplicate', 'stale', 'staleBase', 'foreign', 'pending']) {
  test(`${mode}: no notification`, async () => {
    const h = harness({[mode]: true}); await notify(h); assert.equal(h.posted.length, 0);
  });
}
test('a user cannot spoof the bot dedup marker', async () => {
  const h = harness(); const original = h.github.paginate;
  h.github.paginate = (method, args) => method === 'comments'
    ? [{user: {login: 'someone'}, body: marker(7, sha)}] : original(method, args);
  await notify(h); assert.equal(h.posted.length, 1);
});
test('dry run never writes', async () => {
  const h = harness(); await notify({...h, dryRun: true}); assert.equal(h.posted.length, 0);
});
test('failure comments render PR-controlled names as text and preserve safe job links', async () => {
  const h = harness(); const original = h.github.paginate;
  h.github.paginate = async (method, args) => {
    const values = await original(method, args);
    if (method === 'jobs') values[0] = {...values[0], conclusion: 'failure',
      name: '@someone [click](https://example.com) <!-- spoof -->'};
    return values;
  };
  await notify(h);
  assert.match(h.posted[0].body, /\*\*FAILED\*\*/);
  assert.doesNotMatch(h.posted[0].body, /@someone|\[click\]|\]\(https:\/\/example\.com|<!-- spoof/);
  assert.match(h.posted[0].body, /actions\/runs\/1\/job\/0/);
});
test('workflow and job contract still matches existing CI', () => {
  const fs = require('node:fs');
  const ci = fs.readFileSync(pathFor('../.github/workflows/ci.yml'), 'utf8');
  for (const name of [...ALWAYS, ...FULL].filter(n => !n.startsWith('DOSBox') && !n.startsWith('Reproducibility build'))) {
    assert.ok(ci.includes(`name: ${name}\n`), name);
  }
  const workflow = fs.readFileSync(pathFor('../.github/workflows/ci-notify.yml'), 'utf8');
  assert.match(workflow, /ref: \$\{\{ github.workflow_sha \}\}/);
  assert.match(workflow, /cancel-in-progress: false/);
  assert.doesNotMatch(workflow, /pull_request_target:|schedule:|secrets\.(?!GITHUB_TOKEN)/);
});
function pathFor(relative) { return require('node:path').join(__dirname, relative); }
