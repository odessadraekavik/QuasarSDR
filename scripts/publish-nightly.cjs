const fs = require('node:fs');

module.exports = async ({ github, context, core }) => {
  const { owner, repo } = context.repo;
  const sha = context.sha;
  const tag = 'NIGHTLY';
  const name = 'QuasarSDR-Windows-x64.zip';
  const data = fs.readFileSync(`dist/${name}`);
  const hash = require('node:crypto').createHash('sha256').update(data).digest('hex');
  if (hash !== process.env.RELEASE_SHA256) throw new Error('Artifact checksum mismatch');
  const head = await github.rest.git.getRef({ owner, repo, ref: 'heads/main' });
  if (head.data.object.sha !== sha) {
    core.notice('A newer push superseded this build; NIGHTLY unchanged.');
    return;
  }
  let release;
  try {
    release = (await github.rest.repos.getReleaseByTag({ owner, repo, tag })).data;
  } catch (error) {
    if (error.status !== 404) throw error;
  }
  if (release?.immutable) throw new Error('NIGHTLY must be mutable to publish rolling builds.');
  try {
    await github.rest.git.getRef({ owner, repo, ref: `tags/${tag}` });
    await github.rest.git.updateRef({ owner, repo, ref: `tags/${tag}`, sha, force: true });
  } catch (error) {
    if (error.status !== 404) throw error;
    await github.rest.git.createRef({ owner, repo, ref: `refs/tags/${tag}`, sha });
  }
  const body = [
    '## QuasarSDR · Windows x64 · RX uniquement',
    '',
    'Version de développement automatique : le tag **NIGHTLY** suit le dernier build validé de `main`.',
    'Le titre utilise la version Cargo et la date Europe/Paris. Plusieurs pushes le même jour remplacent le même ZIP.',
    '',
    '### Installation',
    'Décompressez le ZIP puis lancez `quasar-sdr.exe`, en conservant le dossier `drivers/` à côté.',
    'Les trois DLL x64 RTL-SDR sont incluses. Le pilote USB Windows **WinUSB** doit être installé séparément si nécessaire.',
    '',
    `**Commit :** [${sha.slice(0, 7)}](${context.serverUrl}/${owner}/${repo}/commit/${sha})`,
    `**Validation :** [Windows RX](${context.serverUrl}/${owner}/${repo}/actions/runs/${context.runId})`,
    `**SHA-256 du ZIP :** \`${hash}\``,
    '',
    `**Licence QuasarSDR :** [GPL-3.0-only](${context.serverUrl}/${owner}/${repo}/blob/${sha}/LICENSE). Les archives sources ci-dessous correspondent au tag.`,
    '**Runtime :** [RTL-SDR Blog V1.4.0 — distribution et sources](https://github.com/rtlsdrblog/rtl-sdr-blog/releases/tag/V1.4.0).',
    '',
    'Build de développement susceptible de contenir des régressions.',
  ].join('\n');
  if (!release) {
    release = (await github.rest.repos.createRelease({ owner, repo, tag_name: tag,
      target_commitish: sha, name: process.env.RELEASE_NAME, body, draft: true, prerelease: false })).data;
  }
  const assets = await github.paginate(github.rest.repos.listReleaseAssets, {
    owner, repo, release_id: release.id, per_page: 100,
  });
  // Replace only the CI-owned asset; preserve any manually attached files.
  for (const asset of assets.filter(asset => asset.name === name)) {
    await github.rest.repos.deleteReleaseAsset({ owner, repo, asset_id: asset.id });
  }
  await github.rest.repos.uploadReleaseAsset({ owner, repo, release_id: release.id, name, data });
  await github.rest.repos.updateRelease({ owner, repo, release_id: release.id,
    name: process.env.RELEASE_NAME, body, draft: false, prerelease: false, make_latest: 'true' });
  core.notice(`Published ${process.env.RELEASE_NAME} (${tag})`);
};
