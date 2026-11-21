const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function loadQmlScript(name) {
    const file = path.join(__dirname, '..', 'qml', name);
    const source = fs.readFileSync(file, 'utf8').replace(/^\.pragma library\r?$/m, '');
    const context = vm.createContext({qsTr: text => text});
    // Qt replaces all occurrences of the lowest-numbered placeholder in each call.
    vm.runInContext(String.raw`
        String.prototype.arg = function (value) {
            const placeholders = this.match(/%[1-9]\d*/g) || [];
            const number = Math.min(...placeholders.map(token => Number(token.slice(1))));
            return this.replace(/%[1-9]\d*/g, token => Number(token.slice(1)) === number ? String(value) : token);
        };
    `, context);
    vm.runInContext(source, context, {filename: file});
    return context;
}

const health = loadQmlScript('HealthText.js');
for (const status of ['works', 'notCovered', '', undefined]) {
    assert.equal(health.label(status), '');
}
for (const status of ['fixable', 'partlyFixable', 'fixed', 'partlyFixed', 'needsUpdate', 'originalFiles']) {
    assert.ok(health.label(status), `${status} has no health label`);
}
assert.equal(health.label('partlyFixed'), 'Partly fixed');
assert.equal(health.symptoms(['cameraAim', 'crashRisk']), 'Camera and aim, Model data issues');
console.log('PASS mod health labels and symptom names');

const home = loadQmlScript('HomeState.js');
const homeBase = {
    gamePath: '/game', detectedGamePath: '', pendingStatus: 'current',
    appliedProfileId: 'p', activeProfileId: 'p', viewedProfileId: 'p',
};
for (const [change, expected, action] of [
    [{}, 'current', 'Play'],
    [{activeProfileId: 'a', viewedProfileId: 'b', appliedProfileId: 'a', pendingChanges: true}, 'viewed', 'Apply…'],
    [{activeProfileId: 'a', viewedProfileId: 'b', appliedProfileId: 'a'}, 'viewed', 'Apply…'],
    [{activeProfileId: 'a', viewedProfileId: 'b', appliedProfileId: ''}, 'viewed', 'Apply…'],
    [{pendingStatus: 'unexpected'}, 'unknown', 'Review library'],
    [{pendingChanges: true}, 'pending', 'Play'],
    [{externalGameinfoChange: true}, 'external', 'Apply again…'],
    [{gamePath: ''}, 'setup', 'Choose folder…'],
    [{loading: true}, 'loading', 'Loading…'],
    [{recoveryRequired: true}, 'recovery', 'Review library'],
    [{outcomeUnknown: true}, 'recovery', 'Review library'],
    [{pendingStatus: 'selectionRequired'}, 'choices', 'Review library'],
    [{pendingStatus: 'unknown'}, 'unknown', 'Review library'],
    [{error: 'Snapshot could not be checked'}, 'unknown', 'Review library'],
    [{appliedProfileId: 'other', enabledCount: 2}, 'pending', 'Play'],
    [{appliedProfileId: '', enabledCount: 0}, 'pending', 'Play'],
    [{appliedProfileId: '', enabledCount: 2}, 'pending', 'Play'],
    [{appliedProfileId: '', enabledCount: 2, pendingStatus: 'pending'}, 'pending', 'Play'],
]) {
    const state = home.state({...homeBase, ...change});
    assert.equal(state, expected, JSON.stringify(change));
    assert.equal(home.action(state), action, JSON.stringify(change));
}
assert.equal(home.sentence('viewed'), 'Apply the selected profile');
console.log('PASS Home state and action wording');

const launch = loadQmlScript('LaunchPresentation.js');
const replacement = launch.performance({
    overlayAction: 'replace',
    presetName: 'Clean',
    changes: [
        {name: 'IndexBufferPoolSizeMB', from: '32', to: '16'},
        {name: 'X', from: null, to: '1'},
    ],
});
assert.match(replacement, /IndexBufferPoolSizeMB: 32 → 16/);
assert.match(replacement, /X: not set → 1/);
assert.equal(launch.performance({overlayAction: 'preserve'}), '');
console.log('PASS performance review text');

const {heroes} = loadQmlScript('HeroData.js');
const mappingFile = path.join(__dirname, '..', 'backend', 'data', 'hero-mapping.json');
const mapping = JSON.parse(fs.readFileSync(mappingFile, 'utf8'));
assert.ok(mapping.length > 0, 'helper hero mappings are missing');
for (const {internal, display} of mapping) {
    assert.ok(heroes.includes(display), `Picker has no hero for ${internal} (${display})`);
}
console.log('PASS helper-detected heroes are available in the picker');
