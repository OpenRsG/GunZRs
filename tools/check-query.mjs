import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';

const execute = promisify(execFile);
const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const fixtures = await fs.mkdtemp(path.join(os.tmpdir(), 're-query-fixtures-'));
const evidenceRoot = path.join(root, '.local/re/query-check');
await fs.mkdir(evidenceRoot, {recursive: true, mode: 0o700});
const evidence = await fs.mkdtemp(path.join(evidenceRoot, 'run-'));
const strace = (await fs.readFile(path.join(root, '.local/strace-path'), 'utf8')).trim();
const report = {checks: [], inputs: [], limitation: 'Generated ELF applications only; not universal target coverage.'};
const digest = async binary => createHash('sha256').update(await fs.readFile(binary)).digest('hex');

async function compile(binary, name, expression, argument, expected) {
    const source = `${binary}.c`;
    const text = `__attribute__((noinline)) int ${name}(int value) { return ${expression}; }\nint main(void) { return ${name}(${argument}) == ${expected} ? 0 : 1; }\n`;
    await fs.writeFile(source, text);
    await execute(process.env.CC ?? 'cc', ['-O0', '-g', '-fno-inline', '-o', binary, source]);
    await execute(binary, []);
    report.inputs.push({name, sha256: await digest(binary), source: text, runtime_assertion: `${argument} => ${expected}`});
}

async function query(label, binary, name, expectLaunch, multiplier, fresh = false) {
    const before = await digest(binary);
    const traceFile = path.join(evidence, `${report.checks.length}-${label}.execve`);
    const start = performance.now();
    const {stdout, stderr} = await execute(strace, ['-f', '-s', '8192', '-e', 'trace=execve', '-o', traceFile,
        'bash', path.join(root, 'scripts/query.sh'), binary, name, ...(fresh ? ['--fresh'] : [])],
        {cwd: fixtures, timeout: 180000, maxBuffer: 2 ** 20});
    const response = JSON.parse(stdout);
    assert.equal(response.ok, true);
    assert.equal(response.data.subject.digest.sha256, before);
    assert.equal(response.data.normalized_result.procedure.name, name);
    assert.equal(await digest(binary), before, 'Investigation changed the original input');
    const dossier = response.data.normalized_result;
    const parameter = dossier.native_api.parameters[0].name;
    const text = dossier.pseudocode.replace(/\s+/g, '');
    if (multiplier !== null) {
        assert(text.includes(`${parameter}*${multiplier}+7`) || text.includes(`${multiplier}*${parameter}+7`),
            'Returned stale or incorrect multiply/add semantics');
    } else {
        assert(text.includes(`${parameter}^0x5a`) || text.includes(`0x5a^${parameter}`), 'Returned incorrect XOR semantics');
    }
    const trace = await fs.readFile(traceFile, 'utf8');
    assert(trace.includes('execve('), 'Missing actual process trace');
    assert.equal(trace.includes('analyzeHeadless'), expectLaunch, 'Unexpected cache/native-launch behavior');
    const result = {label, status: 'PASS', ms: Math.round(performance.now() - start), sha256: before,
        evidence_id: response.data.evidence_id, native_launch: expectLaunch};
    report.checks.push(result);
    await fs.writeFile(path.join(evidence, `${label}.json`), stdout, {mode: 0o600});
    await fs.writeFile(path.join(evidence, `${label}.stderr`), stderr, {mode: 0o600});
    console.log(`PASS ${label}: ${result.ms} ms, native launch ${expectLaunch}`);
    return result;
}

try {
    const calculator = path.join(fixtures, 'calculator');
    const codec = path.join(fixtures, 'codec');
    await compile(calculator, 'scale', 'value * 3 + 7', 11, 40);
    const first = await query('calculator-first', calculator, 'scale', true, 3);
    await query('calculator-cached', calculator, 'scale', false, 3);
    await query('calculator-fresh', calculator, 'scale', true, 3, true);
    await compile(calculator, 'scale', 'value * 5 + 7', 11, 62);
    const changed = await query('calculator-changed-bytes', calculator, 'scale', true, 5);
    assert.notEqual(changed.sha256, first.sha256, 'Changed fixture must have different bytes');
    await compile(codec, 'decode', 'value ^ 0x5a', 0x5a, 0);
    await query('independent-codec-first', codec, 'decode', true, null);
    await query('independent-codec-cached', codec, 'decode', false, null);
    let failed;
    try {
        await execute('bash', [path.join(root, 'scripts/query.sh'), codec, 'missing_procedure_for_check'],
            {cwd: fixtures, timeout: 180000, maxBuffer: 2 ** 20});
        assert.fail('Unknown procedure unexpectedly succeeded');
    } catch (error) {
        failed = error;
    }
    assert(Number.isInteger(failed.code) && failed.code > 0, 'Failure did not propagate as a nonzero exit');
    assert.equal(JSON.parse(failed.stdout).ok, false);
    const failureRun = /^Evidence: (.+) \(\d+ ms;/m.exec(failed.stderr)?.[1];
    assert(failureRun, 'Failure evidence path missing');
    await assert.rejects(fs.stat(path.join(path.dirname(failureRun), 'snapshot.json')), {code: 'ENOENT'});
    report.checks.push({label: 'unknown-procedure-not-cached', status: 'PASS', exit: failed.code, evidence: failureRun});
    console.log('PASS unknown procedure: error propagated, no reusable snapshot created');
    report.status = 'PASS';
} catch (error) {
    report.status = 'FAIL';
    report.error = error.message;
    console.error(error.stack);
    process.exitCode = 1;
} finally {
    await fs.rm(fixtures, {recursive: true, force: true});
    await fs.writeFile(path.join(evidence, 'report.json'), JSON.stringify(report, null, 2) + '\n', {mode: 0o600});
    console.log(`${report.status} query workflow evidence: ${evidence}`);
}
