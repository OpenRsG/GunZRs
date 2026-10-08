import {createHash} from 'node:crypto';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawn} from 'node:child_process';
import {pipeline} from 'node:stream/promises';

const args = process.argv.slice(2);
if (args.length === 1 && (args[0] === '--help' || args[0] === '-h')) {
    console.log('Usage: bash scripts/query.sh BINARY PROCEDURE [--fresh]');
    process.exit(0);
}
if (args.length < 2 || args.length > 3 || (args.length === 3 && args[2] !== '--fresh')) {
    console.error('Usage: bash scripts/query.sh BINARY PROCEDURE [--fresh]');
    process.exit(2);
}
const [selected, procedure, freshFlag] = args;
const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
let run;
let child;
try {
    const target = await fsp.realpath(selected);
    if (!(await fsp.stat(target)).isFile()) throw new Error('Target must be a regular file');
    const hash = createHash('sha256');
    for await (const bytes of fs.createReadStream(target)) hash.update(bytes);
    const sha256 = hash.digest('hex');
    const query = {operation: 'analyze_function', parameters: {procedure}};
    const question = 'function-' + createHash('sha256').update(JSON.stringify(query)).digest('hex');
    const work = path.join(root, '.local/re', sha256, question);
    await fsp.mkdir(work, {recursive: true, mode: 0o700});
    run = await fsp.mkdtemp(path.join(work, 'run-'));
    const cache = path.join(work, 'snapshot.json');
    const snapshot = path.join(run, 'snapshot.json');
    let suppliedSnapshot = false;
    if (!freshFlag) {
        try {
            await fsp.copyFile(cache, snapshot);
            suppliedSnapshot = true;
        } catch (error) {
            if (error.code !== 'ENOENT') throw error;
        }
    }
    const request = {target, sha256, ...query, fresh: Boolean(freshFlag), supplied_snapshot: suppliedSnapshot,
        started_at: new Date().toISOString()};
    await fsp.writeFile(path.join(run, 'request.json'), JSON.stringify(request, null, 2) + '\n', {mode: 0o600});
    const responsePath = path.join(run, 'response.json');
    const start = performance.now();
    child = spawn('timeout', ['--signal=INT', '--kill-after=10s', '600s', 'bash', path.join(root, 'scripts/rea.sh'),
        'function', target, procedure, '--snapshot', snapshot, '--format', 'json', '--full-output'],
        {cwd: root, stdio: ['ignore', 'pipe', 'pipe']});
    const interrupt = () => child.kill('SIGINT');
    process.once('SIGINT', interrupt);
    process.once('SIGTERM', interrupt);
    const exit = new Promise((resolve, reject) => {
        child.once('error', reject);
        child.once('close', (code, signal) => resolve({code, signal}));
    });
    const output = pipeline(child.stdout, fs.createWriteStream(responsePath, {mode: 0o600}));
    const errors = pipeline(child.stderr, fs.createWriteStream(path.join(run, 'stderr.log'), {mode: 0o600}));
    const [outcome] = await Promise.all([exit, output, errors]);
    process.removeListener('SIGINT', interrupt);
    process.removeListener('SIGTERM', interrupt);
    const elapsed = Math.round(performance.now() - start);
    if ((await fsp.stat(responsePath)).size > 16 * 1024 * 1024) {
        throw new Error('Response exceeds the 16 MiB display limit; full output is retained as evidence, not truncated');
    }
    const text = await fsp.readFile(responsePath, 'utf8');
    const response = JSON.parse(text);
    if (outcome.code !== 0 || response.ok !== true || response.data?.error) {
        const error = response.error ?? (response.data?.error ? response.data : {
            code: 'process_failure', message: `Analysis process exited with code ${outcome.code}, signal ${outcome.signal}`,
        });
        await fsp.writeFile(path.join(run, 'result.json'), JSON.stringify({status: 'failed', ...outcome, ms: elapsed, error}, null, 2) + '\n', {mode: 0o600});
        process.stdout.write(JSON.stringify({ok: false, error, meta: {raw_response: responsePath, ms: elapsed}}, null, 2) + '\n');
        process.exitCode = outcome.code > 0 ? outcome.code : 1;
    } else {
        const evidence = response.data;
        if (evidence.subject?.digest.sha256 !== sha256 || evidence.provider?.id !== 'ghidra' ||
            evidence.operation !== query.operation || evidence.parameters?.procedure !== procedure) {
            throw new Error('Returned evidence does not match the selected input, provider and question; cache was not promoted');
        }
        // The directory names the question; REA validates the actual profile/query cache key.
        const pending = path.join(work, `${path.basename(run)}.snapshot.tmp`);
        await fsp.copyFile(snapshot, pending);
        await fsp.rename(pending, cache);
        await fsp.writeFile(path.join(run, 'result.json'), JSON.stringify({status: 'observed', code: 0, ms: elapsed,
            evidence_id: evidence.evidence_id, provider: evidence.provider, question, sha256}, null, 2) + '\n', {mode: 0o600});
        const dossier = evidence.normalized_result;
        const summary = {
            ok: true,
            data: {
                evidence_id: evidence.evidence_id, subject: evidence.subject, provider: evidence.provider,
                operation: evidence.operation, parameters: evidence.parameters, limitations: evidence.limitations,
                normalized_result: {
                    procedure: {name: dossier.procedure.name, address: dossier.procedure.address, signature: dossier.procedure.signature},
                    pseudocode: dossier.pseudocode,
                    native_api: {available: dossier.native_api?.available ?? null, parameters: dossier.native_api?.parameters ?? []},
                },
            },
            meta: {raw_response: responsePath, snapshot, ms: elapsed, snapshot_supplied: suppliedSnapshot},
        };
        process.stdout.write(JSON.stringify(summary, null, 2) + '\n');
    }
    console.error(`Evidence: ${run} (${elapsed} ms; snapshot supplied: ${suppliedSnapshot})`);
} catch (error) {
    if (child && child.exitCode === null && child.signalCode === null) child.kill('SIGINT');
    if (run) {
        await fsp.writeFile(path.join(run, 'failure.json'), JSON.stringify({error: error.message}, null, 2) + '\n', {mode: 0o600});
        console.error(`Evidence: ${run}`);
    }
    console.error(error.message);
    process.exitCode = 1;
}
