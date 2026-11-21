"""Shared paired-build fixture contract and inert, typed startup reads.

Scenario peers override their own operations before calling startup(). Unexpected
operations remain errors in those peers; this does not simulate game/provider IO.
The literal contract is independent of the application's admission implementation.
"""
import hashlib
import json
from pathlib import Path

FEATURES = json.loads(Path(__file__).with_name('paired-helper-contract.json').read_text())
STARTUP_METHODS = frozenset(('native.game.detect', 'native.installed.scan', 'native.catalog.open', 'native.catalog.status', 'native.catalog.close', 'native.library.inspect', 'native.local.status', 'native.options.status',
    'native.game.status', 'native.launch.owner', 'native.launch.control', 'native.launch.settings.get',
    'native.launch.barriers', 'native.launch.close', 'native.performance.get', 'native.config.get',
    'native.health.get', 'native.health.check'))
_controls = {}
_owner = False
_revision = '1'
_health = None


def hello():
    return dict(protocolVersion=1, readOnly=False, capabilities=list(FEATURES))


def startup(method, p, native=True):
    """Return (handled, value), including empty history without masking jobs."""
    global _owner, _health
    if method == 'native.game.detect':
        return True, dict(detectedGamePath='', candidates=[])
    if method == 'native.installed.scan':
        return True, dict(items=[])
    if method in ('native.catalog.open', 'native.catalog.status', 'native.catalog.close') and not p.get('requestKey'):
        value = dict(version=1, root=p['dataDir'], catalogSession=hashlib.sha256(p['dataDir'].encode()).hexdigest()[:32])
        if method.endswith('close'):
            value['phase'] = 'closed'
        else:
            value.update(phase='failed', error=dict(kind='catalogUnavailable', message='Synthetic catalog is offline.', retryAfterSeconds=None))
        return True, value
    if method in ('native.options.status', 'native.local.status') and not p.get('jobId'):
        return True, []
    if method == 'native.game.status':
        return True, dict(version=1, state='idle', scanComplete=True, processes=[], stop=None)
    if method == 'native.launch.owner':
        return True, dict(version=1, state='open' if _owner else 'closed', busy=False, active=False, childRetained=False)
    if method == 'native.launch.close':
        _owner = False
        return True, dict(version=1, status='closing')
    if method == 'native.launch.control':
        return True, _controls[p['controlId']]
    if method in ('native.launch.open', 'native.launch.barriers', 'native.launch.settings.get'):
        payload = {k: v for k, v in p.items() if k not in ('dataDir', 'controlId')}
        digest = hashlib.sha256(json.dumps(dict(rootPath=p['dataDir'], input=dict(method=method, params=payload)),
                                          sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
        if method == 'native.launch.open':
            _owner = True
            value = dict(version=1, epoch='1', admitted='0', settingsWrites='0')
        elif method == 'native.launch.barriers':
            value = dict(version=1, barriers=[])
        else:
            value = dict(version=1, revision='0', additionalArguments='')
        _controls[p['controlId']] = dict(version=1, controlId=p['controlId'], state='succeeded', inputSha256=digest, value=value, error=None)
        return True, dict(version=1, accepted=True, controlId=p['controlId'], inputSha256=digest)
    if method in ('native.remote.open', 'native.remote.list', 'native.remote.replaceList', 'native.remote.close'):
        value = dict(version=1, root=p['dataDir'], sessionId=p['sessionId'])
        if method.endswith(('list', 'replaceList')):
            value['jobs'] = []
        else:
            value.update(phase='closed' if method.endswith('close') else 'ready', error=None)
        return True, value
    if method == 'native.library.inspect':
        return True, dict(version=1, root=p['dataDir'], state='initialized' if native else 'absent',
                          canCreate=not native, revision=_revision if native else None,
                          reason='existingLibrary' if native else None, candidates=0, blockedPaths=[], pathsTruncated=False)
    if method == 'native.modUpdates.get':
        return True, dict(profileId=p['profileId'], revision=_revision, lastUpdateCheck=None, mods=[])
    if method == 'native.config.get':
        value = dict(path='/synthetic/game/citadel/cfg/'+('autoexec.cfg' if p['kind']=='autoexec' else 'user_hero_settings.vcfg'), canUndo=False, available=True)
        if p['kind'] == 'autoexec':
            value.update(text='', managedSections=[])
        else:
            defaults = dict(gap=0, width=4, height=16, pipOpacity=1, dotOpacity=1, dotOutlineOpacity=1,
                color=dict(r=255,g=255,b=255), outlineColor=dict(r=0,g=0,b=0), pipBorder=False, pipGapStatic=False,
                hero='Default', dotSize=2, dotOutlineWidth=0, dotOutlineGap=0, pipOutlineWidth=0,
                pipOutlineGap=0, pipOutlineOpacity=1, gapCollapse=False, themed=False, overrides={})
            value.update(values=defaults, defaults=defaults, heroScopes=[], heroes=[], ranges={})
        return True, value
    if method == 'native.performance.get':
        return True, dict(selection='', explicitSelection=False, detected=dict(state='stock'), presets=[])
    if method in ('native.health.get', 'native.health.check'):
        if method.endswith('check'): _health = dict(mods=[], gameBuild=0, unavailableMods=[])
        return True, dict(running=False, checked=0, total=0, report=_health)
    return False, None


def apply_review(profile, game='/synthetic/game', count=0, total=0, warnings=None):
    return dict(summaryVersion=2, deploymentLayout='shards-v1', token='a'*32,
                profileId=profile, gamePath=game, packageCount=count, directoryCount=count,
                physicalFileCount=count, companionFileCount=0, fileCount=count, totalBytes=total,
                shardCount=1, shards=[dict(index=1, directoryCount=count)], replacedMounts=[], warnings=warnings or [])


def observe(value):
    global _revision
    if isinstance(value, dict) and value.get('sourceKind') == 'native':
        _revision = value['revision']
