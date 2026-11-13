'use strict';

const {execFileSync} = require('node:child_process');
const path = require('node:path');

// Mirror the existing CI contract, without changing its gates. Unknown jobs
// are also inspected; missing mandatory jobs cannot silently count as success.
const ALWAYS = [
  'Classify change risk', 'Repository guardrails', 'Clippy warning ratchet',
  'R2 filesystem gate', 'Dependency and secret audit', 'Test shard coverage', 'CI gate',
  'DOSBox sandbox enforcement (ubuntu-24.04)',
  'DOSBox sandbox enforcement (ubuntu-latest)',
];
const FULL = [
  'Installer script tests', 'Maintenance script tests', 'Create canonical artifact', 'Verify downloaded artifact',
  'Reproducibility build 1', 'Reproducibility build 2', 'Reproducibility',
];
const UNEXPANDED_BUILD = 'Reproducibility build ${{ matrix.run }}';
const ANALYZE = ['actions', 'c-cpp', 'python', 'rust'].map(x => `Analyze (${x})`);
// One marker per PR, exact head and result. A changed result (e.g. FAILED, rerun,
// then READY) is a new notification; the same result for the same head is not.
const marker = (number, sha, status = 'READY') => `<!-- emuwiz-ci-final:${number}:${sha}:${status} -->`;
const MARKER_RE = /<!-- emuwiz-ci-final:(\d+):([a-f0-9]{40}):(READY|FAILED|BLOCKED) -->/;
const isBot = c => c.user?.login === 'github-actions[bot]';
// Stale-result handling: the first line is rewritten (marker disarmed, mention
// removed) so an old FAILED/READY snapshot can neither dedupe nor mislead. Edits
// do not send email. Only comments authored by the Actions bot are ever touched.
async function supersede({github, repo, comments, keep, core}) {
  for (const c of comments) {
    const m = isBot(c) && c.body?.match(MARKER_RE);
    if (!m || m[0] === keep) continue;
    const body = [`~~Superseded~~ — earlier ${m[3]} snapshot for head \`${m[2]}\` is no longer current.`,
      `<!-- emuwiz-ci-superseded:${m[1]}:${m[2]}:${m[3]} -->`].join('\n');
    try {
      await github.rest.issues.updateComment({...repo, comment_id: c.id, body});
      core.info(`superseded comment ${c.id}`);
    } catch (e) { core.info(`could not supersede comment ${c.id}: ${e.message}`); }
  }
}

function classify(files) {
  if (!files.length || files.length >= 3000) return 'full';
  return execFileSync('bash', [path.join(__dirname, 'classify-ci-changes.sh'),
    ...files.map(f => f.filename)], {encoding: 'utf8'}).trim();
}

function decide({runs, jobs, tier, checks, statuses, mergeable, mergeStateStatus, draft}) {
  if (runs.length !== 2 || runs.some(r => r.status !== 'completed') ||
      jobs.some(j => j.status !== 'completed') ||
      checks.some(c => c.status !== 'completed') ||
      statuses.some(s => s.state === 'pending')) return {status: 'PENDING'};

  const shards = tier === 'gui' ? ['gui-tier-v2', 'gui-tier-other'] : ['recovery', 'dat', 'gui-v2', 'rest'];
  const ciJobs = jobs.filter(j => j.run_id === runs[0].id);
  const classifiedFast = (tier === 'core' || tier === 'gui') && ciJobs.some(j =>
    j.name === 'Classify change risk' && j.conclusion === 'success');
  // Actions evaluates the job-level if before expanding a skipped matrix.
  // Accept only that exact single skipped CI job, never missing full builds.
  const unexpanded = ciJobs.filter(j => j.name === UNEXPANDED_BUILD);
  const collapsedBuilds = classifiedFast && unexpanded.length === 1 &&
    unexpanded[0].conclusion === 'skipped' &&
    !ciJobs.some(j => /^Reproducibility build [12]$/.test(j.name));
  const fullJobs = collapsedBuilds
    ? [...FULL.filter(n => !/^Reproducibility build [12]$/.test(n)), UNEXPANDED_BUILD]
    : FULL;
  const expected = [
    [...ALWAYS, ...fullJobs, ...shards.map(s => `Formatting and relevant tests (${s})`)],
    ANALYZE,
  ];
  const problems = [];
  for (let i = 0; i < runs.length; i++) {
    const runJobs = jobs.filter(j => j.run_id === runs[i].id);
    for (const name of expected[i]) {
      if (!runJobs.some(j => j.name === name)) {
        problems.push({name: `Missing job: ${name}`, url: runs[i].html_url});
      }
    }
    if (runs[i].conclusion !== 'success') {
      problems.push({name: `${runs[i].name}: ${runs[i].conclusion}`, url: runs[i].html_url});
    }
  }
  // Only the seven full-tier jobs may skip, only on core/gui PRs,
  // and only when classification itself succeeded. Never allowlist by name alone.
  const expectedSkip = j => classifiedFast && j.run_id === runs[0].id &&
    (FULL.includes(j.name) || (collapsedBuilds && j.name === UNEXPANDED_BUILD));
  const allowedCheckIds = new Set(jobs.filter(j => expectedSkip(j) &&
    j.conclusion === 'skipped').map(j => Number(j.check_run_url.split('/').pop())));
  for (const j of jobs) {
    if (j.conclusion !== 'success' && !(j.conclusion === 'skipped' && expectedSkip(j))) {
      problems.push({name: `${j.name}: ${j.conclusion}`, url: j.html_url});
    }
  }
  for (const c of checks) {
    if (c.conclusion !== 'success' &&
        !(c.conclusion === 'skipped' && allowedCheckIds.has(c.id))) {
      problems.push({name: `${c.name}: ${c.conclusion}`, url: c.html_url});
    }
  }
  for (const s of statuses) {
    if (s.state !== 'success') problems.push({name: `${s.context}: ${s.state}`, url: s.target_url});
  }
  if (problems.length) return {status: 'FAILED', problems};
  // CodeQL's analysis-result check can finish separately from its Actions jobs.
  // A failed/cancelled analysis may never produce this extra check.
  if (!checks.some(c => c.name === 'CodeQL' && c.app?.slug === 'github-advanced-security')) {
    return {status: 'PENDING'};
  }
  if (draft || mergeable !== 'MERGEABLE' || mergeStateStatus !== 'CLEAN') {
    return {status: 'BLOCKED', draft: !!draft, problems: [{name:
      `Checks passed; mergeability: ${mergeable}/${mergeStateStatus}${draft ? ' (draft)' : ''}`} ]};
  }
  return {status: 'READY', problems: []};
}

async function notify({github, context, core, dryRun = false}) {
  const repo = context.repo;
  const p = context.payload;
  const sha = p.workflow_run?.head_sha || p.check_run?.head_sha || p.inputs?.head_sha;
  if (!/^[a-f0-9]{40}$/.test(sha || '')) throw new Error('Expected a full head SHA');
  const all = (method, args) => github.paginate(method, {...repo, per_page: 100, ...args});
  // Default CodeQL setup has empty pull_requests arrays; do not trust payload
  // associations (or branch names), particularly for fork PRs.
  const prs = (await all(github.rest.pulls.list, {state: 'open'}))
    .filter(pr => pr.head.sha === sha);
  for (const pr of prs) {
    const comments = await all(github.rest.issues.listComments, {issue_number: pr.number});
    const runs = [];
    const workflows = await all(github.rest.actions.listRepoWorkflows, {});
    for (const workflow_id of ['ci.yml', 'dynamic/github-code-scanning/codeql']) {
      // Resolve dynamic CodeQL by its trusted workflow path, not its display name.
      const workflow = workflows.find(w => w.path === (workflow_id === 'ci.yml'
        ? '.github/workflows/ci.yml' : workflow_id));
      if (!workflow) continue;
      const candidates = await all(github.rest.actions.listWorkflowRuns, {
        workflow_id: workflow.id, head_sha: sha,
      });
      const run = candidates.filter(r => r.head_sha === sha &&
        (workflow_id === 'ci.yml' ? r.event === 'pull_request' : r.event === 'dynamic'))
        .sort((a, b) => b.id - a.id)[0];
      if (run) runs.push(run);
    }
    if (runs.length !== 2) { core.info(`PR #${pr.number}: PENDING (missing workflow)`);
      if (!dryRun) await supersede({github, repo, comments, keep: null, core});
      continue; }
    const jobs = [];
    for (const run of runs) {
      jobs.push(...await all(github.rest.actions.listJobsForWorkflowRun, {
        run_id: run.id, filter: 'latest',
      }));
    }
    const files = await all(github.rest.pulls.listFiles, {pull_number: pr.number});
    const checks = await all(github.rest.checks.listForRef, {ref: sha, filter: 'latest'});
    // The REST endpoint returns status history; only the latest per context counts.
    const history = await all(github.rest.repos.listCommitStatusesForRef, {ref: sha});
    const statuses = [...new Map(history.slice().reverse().map(s => [s.context, s])).values()];
    const {repository} = await github.graphql(`query($owner:String!, $repo:String!, $number:Int!) {
      repository(owner:$owner, name:$repo) { pullRequest(number:$number) {
        headRefOid baseRefOid state isDraft mergeable mergeStateStatus
      } }
    }`, {owner: repo.owner, repo: repo.repo, number: pr.number});
    const current = repository.pullRequest;
    if (current.headRefOid !== sha || current.state !== 'OPEN') continue;
    const result = decide({runs, jobs, tier: classify(files), checks, statuses,
      mergeable: current.mergeable, mergeStateStatus: current.mergeStateStatus, draft: current.isDraft});
    // Quiet states: log only, never mention. A draft is work in progress, and an
    // UNKNOWN mergeability is GitHub still computing. Any earlier snapshot is stale.
    const quiet = result.status === 'PENDING' || (result.status === 'BLOCKED' &&
      (result.draft || current.mergeable === 'UNKNOWN' || current.mergeStateStatus === 'UNKNOWN'));
    if (quiet) {
      core.info(`PR #${pr.number}: ${result.status}${result.draft ? ' (draft)' : ''} - no notification`);
      if (!dryRun) await supersede({github, repo, comments, keep: null, core});
      continue;
    }
    const key = marker(pr.number, sha, result.status);
    if (comments.some(c => isBot(c) && c.body?.includes(key))) {
      if (!dryRun) await supersede({github, repo, comments, keep: key, core});
      continue;
    }
    // Recheck immediately before writing; a stale event must never bless a new head.
    const {data: fresh} = await github.rest.pulls.get({...repo, pull_number: pr.number});
    if (fresh.head.sha !== sha || fresh.base.sha !== current.baseRefOid || fresh.state !== 'open') continue;
    // Names can originate in a PR; prevent injected mentions, links or markers.
    const safe = value => String(value).replace(/[^a-zA-Z0-9 :/()._-]/g, '?').slice(0, 200);
    const safeUrl = value => /^https:\/\/github\.com\//.test(value || '')
      ? new URL(value).href.replace(/[()]/g, c => c === '(' ? '%28' : '%29') : pr.html_url;
    const links = runs.map(r => `- [${safe(r.name)}](${safeUrl(r.html_url)})`);
    const details = result.problems.map(x => x.url
      ? `- [${safe(x.name)}](${safeUrl(x.url)})` : `- ${safe(x.name)}`);
    const body = [
      key, `@kiehntre **${result.status}** — EmuWiz PR #${pr.number}`,
      '', `Head: \`${sha}\``, `PR: ${pr.html_url}`, '', ...details, ...links,
      '', 'Snapshot for this exact head and result. A changed result posts a new notification and marks this one superseded.',
      'Merge approval and a fresh exact-head merge-readiness check are still required.',
    ].join('\n');
    if (dryRun) { core.info(body); continue; }
    await github.rest.issues.createComment({...repo, issue_number: pr.number, body});
    await supersede({github, repo, comments, keep: key, core});
  }
}

module.exports = {ALWAYS, FULL, ANALYZE, marker, classify, decide, notify};
