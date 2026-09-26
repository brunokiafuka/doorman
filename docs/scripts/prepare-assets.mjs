import { copyFile, mkdir } from 'node:fs/promises';
const publicDir = new URL('../public/', import.meta.url);
await mkdir(new URL('images/', publicDir), { recursive: true });
await copyFile(new URL('../../install.sh', import.meta.url), new URL('install.sh', publicDir));
await copyFile(new URL('../images/traffic-inspector.png', import.meta.url), new URL('images/traffic-inspector.png', publicDir));

// Share the desktop app's licensed fonts instead of maintaining another copy.
await mkdir(new URL('fonts/', publicDir), { recursive: true });
for (const file of ['Geist-Regular.ttf', 'Geist-Medium.ttf', 'Geist-SemiBold.ttf', 'Geist-Bold.ttf', 'Geist-OFL.txt']) {
  await copyFile(new URL(`../../apps/doorman-desktop/assets/fonts/${file}`, import.meta.url), new URL(`fonts/${file}`, publicDir));
}
