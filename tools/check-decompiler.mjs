import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {Client} from '@modelcontextprotocol/client';
import {StdioClientTransport} from '@modelcontextprotocol/client/stdio';

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const execute = promisify(execFile);
const [elf, pe, source, sourceHash, ...buildTimes] = process.argv.slice(2);
assert(elf && pe && source && /^[a-f0-9]{64}$/.test(sourceHash), 'Run bash scripts/check-decompiler.sh');
const evidenceRoot = path.join(root, '.local/re/engine-check');
await fs.mkdir(evidenceRoot, {recursive: true, mode: 0o700});
const output = await fs.mkdtemp(path.join(evidenceRoot, 'run-'));
const report = {started_at: new Date().toISOString(), checks: [], binaries: [], limitations: [
    'Known-source probes only; coverage does not extend to untested applications or architectures.',
    'PE DLL is statically analyzed, never executed; it has no CRT, entry point or internal callers.',
    'Static references do not prove runtime execution; indirect calls and original-source parity are outside this check.',
]};
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const sourceText = await fs.readFile(source, 'utf8');
assert.equal(hash(sourceText), sourceHash, 'Source identity mismatch');
report.source = {sha256: sourceHash, text: sourceText};
for (const [index, name] of ['ELF compile', 'ELF runtime assertion 11 => 40', 'PE compile', 'PE link (not executed)'].entries()) {
    const ms = Number(buildTimes[index]);
    assert(Number.isFinite(ms) && ms >= 0, 'Missing shell prerequisite timing');
    report.checks.push({name, status: 'PASS', ms, evidence: 'Successful shell prerequisite'});
}
const client = new Client({name: 're-framework-decompiler-check', version: '1.0.0'}, {capabilities: {}});
const transport = new StdioClientTransport({command: 'bash', args: [path.join(root, 'scripts/rea.sh'), 'mcp'],
    cwd: root, env: {...process.env}, stderr: 'pipe'});
let stderr = '';
transport.stderr?.on('data', chunk => { stderr = (stderr + chunk.toString()).slice(-65536); });
let active = false;
let currentHash;
let calls = 0;
const warmMs = 60000;
const importMs = 600000;

async function check(name, action) {
    const start = performance.now();
    try {
        const detail = await action();
        const result = {name, status: 'PASS', ms: Math.round(performance.now() - start), ...(detail ? {detail} : {})};
        report.checks.push(result);
        console.log(`PASS ${name} (${result.ms} ms)${detail ? `: ${detail}` : ''}`);
        return detail;
    } catch (error) {
        const result = {name, status: 'FAIL', ms: Math.round(performance.now() - start), error: error.message};
        report.checks.push(result);
        console.error(`FAIL ${name} (${result.ms} ms): ${error.message}`);
        throw error;
    }
}
async function call(name, args = {}, timeout = warmMs) {
    const raw = await client.callTool({name, arguments: args}, {timeout, maxTotalTimeout: timeout});
    const payload = raw.structuredContent ?? JSON.parse(raw.content.filter(item => item.type === 'text').map(item => item.text).join('\n'));
    await fs.writeFile(path.join(output, `${String(++calls).padStart(2, '0')}-${name}.json`),
        JSON.stringify({name, arguments: args, isError: raw.isError ?? false, payload}, null, 2) + '\n', {mode: 0o600});
    assert(!raw.isError, `${name}: ${JSON.stringify(payload)}`);
    assert(Object.hasOwn(payload, 'result'), `${name}: missing structured result`);
    if (payload.evidence) {
        assert.equal(payload.evidence.provider.id, 'ghidra', `${name}: wrong provider`);
        assert.equal(payload.evidence.subject?.digest.sha256, currentHash, `${name}: wrong binary identity`);
    }
    return payload.result;
}
function verifyPseudocode(text, parameter) {
    assert.equal(typeof text, 'string', 'Missing pseudocode');
    assert(parameter && /^[A-Za-z_][A-Za-z_0-9]*$/.test(parameter), 'Missing input parameter identity');
    const normalize = expression => expression
        .replace(/\((?:u?int|u?long|longlong|ulonglong|undefined[1248]|unsigned(?:\s+int)?|signed(?:\s+int)?)\)/g, '')
        .replace(/[\s()]/g, '').replace(/0x0*3\b/g, '3').replace(/0x0*7\b/g, '7');
    const expected = new Set([`${parameter}*3+7`, `3*${parameter}+7`, `7+${parameter}*3`, `7+3*${parameter}`]);
    const returns = [...text.matchAll(/\breturn\s+([^;]+);/g)].map(match => normalize(match[1]));
    const assignments = [...text.matchAll(/\b([A-Za-z_][A-Za-z_0-9]*)\s*=\s*([^;]+);/g)];
    assert(returns.some(value => expected.has(value)) || assignments.some(match =>
        expected.has(normalize(match[2])) && returns.includes(match[1])),
    `Pseudocode lacks returned input * 3 + 7 (parameter ${parameter})`);
}
const address = value => { assert(/^0x[0-9a-f]+$/i.test(value), `Invalid address ${value}`); return BigInt(value); };
function inside(value, procedure) {
    return procedure.body.ranges.some(range => address(value) >= address(range.start) && address(value) <= address(range.end));
}
function lineage(session) {
    assert(session.open && session.analysis_run?.run_id, 'Missing active analysis run');
    assert.equal(session.analysis_provider_binding?.provider.id, 'ghidra', 'Missing Ghidra binding');
    const observation = session.analysis_run.process_lineage.snapshots?.find(item => item.provider.id === 'ghidra')?.observation;
    assert.equal(observation?.status, 'verified', 'Missing token-verified process lineage');
    assert(Number.isInteger(observation.launcher_pid) && observation.launcher_pid > 0, 'Missing launcher PID');
    assert.equal(observation.launcher_parent_pid, transport.pid, 'Launcher belongs to another MCP process');
    return {run_id: session.analysis_run.run_id, launcher_pid: observation.launcher_pid,
        process_group_id: observation.process_group_id, observation};
}
async function ownedProject(session) {
    const live = lineage(session);
    const pids = [live.launcher_pid, ...live.observation.descendants.map(item => item.pid)];
    for (const pid of pids) {
        let command;
        try { command = (await fs.readFile(`/proc/${pid}/cmdline`, 'utf8')).split('\0'); }
        catch (error) { if (error.code === 'ENOENT') continue; throw error; }
        const index = command.indexOf('rea-project');
        if (index < 1 || !command.includes('-import')) continue;
        const project = command[index - 1];
        assert(path.isAbsolute(project), 'Observed project path must be absolute');
        const runtime = path.dirname(project);
        assert(/^rea-ghidra-/.test(path.basename(runtime)), 'Observed path is not a private REA Ghidra root');
        const ownership = JSON.parse(await fs.readFile(path.join(runtime, 'ownership.json'), 'utf8'));
        assert.equal(ownership.run_id, live.run_id, 'Project ownership run mismatch');
        assert.equal(ownership.pid, live.launcher_pid, 'Project ownership PID mismatch');
        assert.equal(ownership.parent_pid, transport.pid, 'Project ownership parent mismatch');
        assert.equal(ownership.process_group_id, live.process_group_id, 'Project ownership group mismatch');
        assert.equal(ownership.ownership_kind, 'posix-process-group');
        assert((await fs.stat(project)).isDirectory(), 'Owned project directory missing before close');
        return {project, runtime, command_pid: pid, ownership, ...live};
    }
    assert.fail('No project argument observed in this session-reported process lineage');
}
async function absent(file) {
    try { await fs.stat(file); } catch (error) { if (error.code === 'ENOENT') return; throw error; }
    assert.fail(`Owned resource still exists after close: ${file}`);
}
async function release(label, owned, snapshot) {
    await check(`${label}: close_binary releases session and owned project`, async () => {
        await call('close_binary', {snapshot_path: snapshot});
        active = false;
        const session = await call('binary_session');
        assert.equal(session.open, false);
        assert.equal(session.analysis_run, null);
        await absent(owned.project);
        await absent(owned.runtime);
        return `run_id/PID/parent/process-group verified; removed ${owned.project}`;
    });
}

try {
    await check('MCP initialize and supported tools', async () => {
        await client.connect(transport, {timeout: warmMs});
        const {tools} = await client.listTools({}, {timeout: warmMs});
        const required = ['open_binary', 'close_binary', 'binary_session', 'analyze_function', 'procedure_pseudo_code',
            'procedure_assembly', 'search_strings', 'xrefs', 'resolve_containing_procedure', 'procedure_references', 'procedure_callees'];
        for (const name of required) assert(tools.some(tool => tool.name === name), `Unsupported critical tool: ${name}`);
        report.tools = tools.filter(tool => required.includes(tool.name)).map(({name, inputSchema}) => ({name, inputSchema}));
        return `${required.length} actual tool schemas retained`;
    });
    for (const [label, binary, format] of [['ELF', elf, 'elf'], ['PE DLL', pe, 'pe']]) {
        currentHash = hash(await fs.readFile(binary));
        const identity = {label, format, sha256: currentHash};
        report.binaries.push(identity);
        await check(`${label}: open exact generated x86-64 binary`, async () => {
            const opened = await call('open_binary', {path: binary, provider_id: 'ghidra'}, importMs);
            active = true;
            assert.equal(opened.sha256, currentHash);
            assert.equal(opened.format, format);
            assert.equal(opened.architecture, 'x86_64');
        });
        let dossier;
        let parameter;
        await check(`${label}: first import and re_probe multiply/add pseudocode`, async () => {
            dossier = await call('analyze_function', {procedure: 're_probe'}, importMs);
            assert.equal(dossier.procedure.name, 're_probe');
            assert(dossier.procedure.body.available && dossier.procedure.body.contains_entry, 'Incomplete function body');
            assert(dossier.native_api?.available && dossier.native_api.parameters.length === 1, 'Missing input signature');
            parameter = dossier.native_api.parameters[0].name;
            verifyPseudocode(dossier.pseudocode, parameter);
            return 'returned input * 3 + 7; original source spelling not required';
        });
        const before = await call('binary_session');
        const owned = await ownedProject(before);
        identity.runtime = {run_id: owned.run_id, launcher_pid: owned.launcher_pid, project: owned.project,
            authority: owned.ownership, observed_command_pid: owned.command_pid};
        await check(`${label}: assembly addresses cover re_probe`, async () => {
            const assembly = await call('procedure_assembly', {procedure: 're_probe'});
            const lines = assembly.trim().split('\n');
            assert(lines.length > 1 && lines.every(line => /^0x[0-9a-f]+: /i.test(line)), 'Invalid assembly rows');
            assert.equal(address(lines[0].split(':')[0]), address(dossier.procedure.address));
            assert(lines.every(line => inside(line.split(':')[0], dossier.procedure)), 'Assembly escapes re_probe body');
            assert(/\bRET\b/i.test(assembly), 'Missing return instruction');
            assert(/\bJ(?:E|NE|Z|NZ)\b/i.test(assembly), 'Missing deterministic marker branch');
            return `${lines.length} instructions in observed re_probe body`;
        });
        await check(`${label}: string -> xref -> re_probe`, async () => {
            const strings = await call('search_strings', {pattern: 're_probe_marker', mode: 'literal', case_sensitive: true});
            const marker = strings.find(item => item.value === 're_probe_marker');
            assert(marker, 'Exact marker string missing');
            const xrefs = await call('xrefs', {address: marker.address});
            const xref = xrefs.find(value => inside(value, dossier.procedure));
            assert(xref, 'Marker has no xref inside re_probe');
            const containing = await call('resolve_containing_procedure', {address: xref});
            assert.equal(containing.found, true);
            assert.equal(containing.procedure.name, 're_probe');
            assert.equal(address(containing.procedure.address), address(dossier.procedure.address));
            identity.marker = {address: marker.address, xref, function_address: containing.procedure.address};
            return `${marker.address} -> ${xref} -> re_probe`;
        });
        await check(`${label}: function references and leaf callees`, async () => {
            const outgoing = await call('procedure_references', {procedure: 're_probe', direction: 'outgoing'});
            const callees = await call('procedure_callees', {procedure: 're_probe'});
            assert.equal(outgoing.reference_kinds_available, true);
            assert(outgoing.references.some(edge => edge.target_address === identity.marker.address &&
                inside(edge.source_address, dossier.procedure) && edge.kind.data), 'Missing marker data reference');
            assert.equal(outgoing.unresolved_calls.length, 0);
            assert.equal(callees.length, 0, 'Leaf probe unexpectedly calls another function');
            assert(!outgoing.references.some(edge => edge.kind.call), 'Leaf probe has call references');
            if (format === 'elf') {
                const incoming = await call('procedure_references', {procedure: 're_probe', direction: 'incoming'});
                assert(incoming.references.some(edge => edge.kind.call && edge.source_procedure?.name === 'main' &&
                    address(edge.target_address) === address(dossier.procedure.address)), 'Missing main -> re_probe call reference');
                const mainCallees = await call('procedure_callees', {procedure: 'main'});
                assert(mainCallees.some(value => address(value) === address(dossier.procedure.address)), 'main callees omit re_probe');
            }
            return format === 'elf' ? 'main -> re_probe; marker data reference; re_probe is a leaf' : 'marker data reference; exported re_probe is a leaf (no internal caller expected)';
        });
        await check(`${label}: warm pseudocode`, async () => {
            verifyPseudocode(await call('procedure_pseudo_code', {procedure: 're_probe'}), parameter);
        });
        await check(`${label}: repeat query retains imported runtime`, async () => {
            const prior = lineage(await call('binary_session'));
            verifyPseudocode(await call('procedure_pseudo_code', {procedure: 're_probe'}), parameter);
            const after = lineage(await call('binary_session'));
            for (const field of ['run_id', 'launcher_pid', 'process_group_id']) {
                assert.equal(after[field], prior[field], `Repeat restarted ${field}`);
                assert.equal(after[field], owned[field], `Warm calls changed ${field}`);
            }
            const stillOwned = await ownedProject(await call('binary_session'));
            assert.equal(stillOwned.project, owned.project, 'Repeat imported a new project');
            return `stable run ${after.run_id}, launcher PID ${after.launcher_pid}, process group ${after.process_group_id}`;
        });
        const snapshot = path.join(output, `${format}.snapshot.json`);
        identity.snapshot = snapshot;
        await release(label, owned, snapshot);
        await check(`${label}: exact CLI snapshot replay without native launch`, async () => {
            const tracePath = path.join(output, `${format}-cache.execve`);
            const strace = process.env.STRACE ?? (await fs.readFile(path.join(root, '.local/strace-path'), 'utf8')).trim();
            const {stdout, stderr: cliStderr} = await execute(strace, [
                '-f', '-s', '8192', '-e', 'trace=execve', '-o', tracePath,
                'bash', path.join(root, 'scripts/rea.sh'), 'function', binary, 're_probe',
                '--snapshot', snapshot, '--format', 'json', '--full-output',
            ], {cwd: root, timeout: warmMs, maxBuffer: 2 ** 20});
            await fs.writeFile(path.join(output, `${format}-cache.json`), stdout, {mode: 0o600});
            await fs.writeFile(path.join(output, `${format}-cache.stderr`), cliStderr, {mode: 0o600});
            const cached = JSON.parse(stdout);
            assert.equal(cached.ok, true);
            assert.equal(cached.data.subject.digest.sha256, currentHash);
            assert.equal(cached.data.provider.id, 'ghidra');
            assert.equal(cached.data.operation, 'analyze_function');
            assert.deepEqual(cached.data.parameters, {procedure: 're_probe'});
            verifyPseudocode(cached.data.normalized_result.pseudocode, parameter);
            const trace = await fs.readFile(tracePath, 'utf8');
            assert(trace.includes('execve('), 'Process trace unavailable');
            assert(!trace.includes('analyzeHeadless'), 'Cache replay started Ghidra');
            return 'same input/query semantics; execve trace shows no analyzeHeadless launch';
        });
    }
} catch (error) {
    report.error = error.message;
    process.exitCode = 1;
} finally {
    if (active) {
        try { await check('Failure cleanup: close active binary', async () => { await call('close_binary'); active = false; }); }
        catch (error) { report.cleanup_error = error.message; process.exitCode = 1; }
    }
    await check('MCP transport close', async () => {
        const pid = transport.pid;
        await client.close();
        assert.equal(transport.pid, null, 'MCP child retained after client.close');
        if (pid) await absent(`/proc/${pid}`);
    }).catch(error => { report.transport_error = error.message; process.exitCode = 1; });
    report.finished_at = new Date().toISOString();
    report.status = process.exitCode ? 'FAIL' : 'PASS';
    await fs.writeFile(path.join(output, 'report.json'), JSON.stringify(report, null, 2) + '\n', {mode: 0o600});
    await fs.writeFile(path.join(output, 'rea-stderr.log'), stderr, {mode: 0o600});
    console.log(`${report.status} engine check evidence: ${output}`);
}
