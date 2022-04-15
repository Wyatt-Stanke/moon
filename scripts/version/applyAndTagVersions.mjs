import fs from 'fs/promises';
import chalk from 'chalk';
import { exec } from '../helpers.mjs';

async function getPackageVersions() {
	const files = await fs.readdir('./packages');
	const versions = {};

	await Promise.all(
		files.map(async (file) => {
			const pkg = JSON.parse(await fs.readFile(`./packages/${file}/package.json`, 'utf8'));

			versions[pkg.name] = pkg.version;
		}),
	);

	return versions;
}

function logDiff(diff) {
	console.log(`Found ${diff.length} packages to release`);

	diff.forEach((row) => {
		console.log(chalk.gray(`  - ${row}`));
	});
}

async function createCommit(versions) {
	console.log('Creating git commit');

	let commit = 'Release';

	versions.forEach((version) => {
		commit += `\n- ${version}`;
	});

	await exec('git', ['add', '--all']);
	await exec('git', ['commit', '-m', `'${commit}'`]);
}

async function createTags(versions) {
	console.log('Creating git tags');

	await Promise.all(
		versions.map(async (version) => {
			await exec('git', ['tag', version]);
		}),
	);
}

async function run() {
	// Gather the versions before we apply the new ones
	const prevVersions = await getPackageVersions();

	// Apply them via yarn
	await exec('yarn', ['version', 'apply', '--all']);

	// Now gather the versions again so we can diff
	const nextVersions = await getPackageVersions();

	console.log(prevVersions, nextVersions);

	// Diff the versions and find the new ones
	const diff = [];

	Object.entries(nextVersions).forEach(([name, version]) => {
		if (version !== prevVersions[name]) {
			diff.push(`${name}@${version}`);
		}
	});

	if (diff.length === 0) {
		console.log(chalk.yellow('No packages to release'));
		return;
	}

	logDiff(diff);

	// Create git commit and tags
	await createCommit(diff);
	await createTags(diff);

	console.log(chalk.green('Created commit and tags!'));
}

run().catch((error) => {
	console.error(error);
	process.exitCode = 1;
});