# Distant

Prise en main à distance minimaliste, façon TeamViewer ou Supremo, écrite en Rust avec [Tauri 2](https://tauri.app).
Windows, macOS et Linux (X11).

- **ID à 9 chiffres + mot de passe**, comme les outils du marché.
- **Relais auto-hébergé** : un petit serveur Rust met les postes en relation, sans compte ni service tiers.
- **Passe les réseaux filtrés** : le relais se joint aussi par le port 443 (`wss://`), comme une page web.
- **Chiffrement de bout en bout** : le relais ne voit ni l'écran, ni les frappes, ni le mot de passe.
- **Accès sans surveillance** : mot de passe permanent, lancement au démarrage, icône dans la zone de notification.
- Plusieurs écrans, trois niveaux de qualité d'image, clavier en touches physiques (AZERTY ↔ AZERTY sans surprise).
- **Envoi de fichiers** vers le poste contrôlé, par glisser-déposer ou depuis la barre d'outils.
- **Mises à jour automatiques** : un poste sans surveillance reçoit les correctifs sans personne devant.

## Architecture

```
 ┌──────────────┐        TCP         ┌──────────────┐        TCP         ┌──────────────┐
 │ Visualiseur  │ ─────────────────▶ │    Relais    │ ◀───────────────── │    Hôte      │
 │ (app Tauri)  │  flux chiffré E2E  │ distant-relay│  flux chiffré E2E  │ (app Tauri)  │
 └──────────────┘                    └──────────────┘                    └──────────────┘
```

| Dossier | Rôle |
|---|---|
| `crates/proto` | Protocole partagé : trames, messages du relais, messages de session, canal chiffré |
| `crates/relay` | Serveur relais (binaire `distant-relay`) |
| `src-tauri` | Application : côté hôte (capture, injection clavier/souris) et côté visualiseur |
| `ui` | Interface (HTML/CSS/JS sans dépendance) |

### Déroulé d'une session

1. L'hôte garde une connexion « contrôle » ouverte vers le relais avec son ID.
2. Le visualiseur demande cet ID ; le relais prévient l'hôte, qui ouvre une connexion dédiée.
3. Le relais relie les deux connexions et recopie les octets, sans les comprendre.
4. Échange **SPAKE2** à partir du mot de passe (dérivé par **PBKDF2**, salé par l'ID) → clés **ChaCha20-Poly1305**, une par sens.
5. L'hôte envoie l'écran en **tuiles JPEG** de 128 px : seules les zones modifiées partent. Le visualiseur accuse réception de chaque image, l'hôte n'en a jamais plus de deux d'avance : la cadence s'adapte au débit et la latence ne s'accumule pas.
6. Les fichiers envoyés par le visualiseur empruntent le même canal chiffré, par morceaux de 64 Ko qui passent après le clavier et la souris.

## Prérequis

- [Rust](https://rustup.rs) 1.85 ou plus récent
- [Node.js](https://nodejs.org) 18+ (uniquement pour la CLI Tauri)
- Les [prérequis système de Tauri](https://tauri.app/start/prerequisites/) :
  - **Windows** : WebView2 (déjà présent sur Windows 10/11) et les outils de build MSVC
  - **macOS** : Xcode Command Line Tools
  - **Linux** : `libwebkit2gtk-4.1-dev`, `libxdo-dev`, `libayatana-appindicator3-dev`, `libxcb1-dev`, etc.

```bash
npm install
```

## Lancer en local

Dans un premier terminal, le relais :

```bash
npm run relay
```

Dans un second, l'application :

```bash
npm run dev
```

Par défaut l'application utilise le relais `127.0.0.1:21116`. Vous pouvez vous connecter à votre propre ID pour essayer (l'image se répète à l'infini, c'est normal).

Pour simuler deux postes sur la même machine, lancez une seconde instance avec un autre profil (autre ID, autres réglages) une fois l'application compilée :

```bash
DISTANT_PROFILE=b ./target/debug/distant
```

## Héberger le relais

Le relais est un simple binaire sans dépendance. Chaque version publiée en fournit un pour Linux (`distant-relay-x86_64-linux`, statique : il tourne sur n'importe quelle distribution) ; sinon, compilez-le :

```bash
cargo build -p distant-relay --release
./target/release/distant-relay --listen 0.0.0.0:21116 --key un-secret-partage
```

- Ouvrez le port TCP **21116** sur le serveur.
- `--key` (ou `DISTANT_RELAY_KEY`) est optionnel : sans clé, n'importe qui connaissant l'adresse peut utiliser votre relais (sans pouvoir lire les sessions des autres). Avec une clé, seules les applications configurées avec cette clé sont acceptées.
- Les journaux se règlent avec `RUST_LOG` (par exemple `RUST_LOG=distant_relay=debug`).

Exemple de service systemd :

```ini
[Unit]
Description=Relais Distant
After=network.target

[Service]
ExecStart=/usr/local/bin/distant-relay --listen 0.0.0.0:21116
Environment=DISTANT_RELAY_KEY=un-secret-partage
Restart=always
DynamicUser=yes

[Install]
WantedBy=multi-user.target
```

Dans l'application, renseignez ensuite l'adresse (`relais.exemple.fr` ou `relais.exemple.fr:21116`) et la clé dans les **réglages**.

## Relais derrière un serveur web (port 443)

Certains réseaux (entreprises, écoles, hôtels) ne laissent sortir que le web : le port 21116 y est bloqué et le relais semble injoignable (« délai dépassé »). Le relais sait donc aussi recevoir ses connexions en **WebSocket**, à travers le serveur web qui tient déjà le port 443. Sur le réseau, la connexion ressemble à n'importe quelle page HTTPS.

Le relais reconnaît seul le mode de chaque connexion, sur le même port : rien à changer à son lancement. S'il n'est joint que par le serveur web, faites-le écouter en local (`--listen 127.0.0.1:21116`).

Avec **nginx**, sur un sous-domaine dédié muni d'un certificat :

```nginx
location / {
    proxy_pass http://127.0.0.1:21116;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header X-Real-IP $remote_addr;
    proxy_read_timeout 300s;
    proxy_send_timeout 300s;
    proxy_buffering off;
}
```

Avec **Caddy** : `relais.exemple.fr { reverse_proxy 127.0.0.1:21116 }`.

Avec **Docker**, une image Alpine suffit : le conteneur télécharge le binaire du relais dans la version voulue, vérifie son empreinte, puis le lance sous un utilisateur sans droits. Le port n'est publié qu'en local, c'est le serveur web qui l'ouvre au monde. L'empreinte est dans `distant-relay-x86_64-linux.sha256`, à côté du binaire dans chaque release.

```yaml
services:
  relay:
    image: alpine:3.24.4
    restart: unless-stopped
    user: "65534:65534"
    working_dir: /relais
    environment:
      DISTANT_RELAY_KEY: "un-secret-partage"
      RELAY_URL: https://github.com/INFOTECH974/open-remote/releases/download/v0.3.0/distant-relay-x86_64-linux
      RELAY_SHA256: "empreinte-sha256-du-binaire"
    command:
      - sh
      - -c
      - |
        set -e
        wget -qO distant-relay "$$RELAY_URL"
        echo "$$RELAY_SHA256  distant-relay" | sha256sum -c -
        chmod +x distant-relay
        exec ./distant-relay --listen 0.0.0.0:21116
    ports:
      - "127.0.0.1:21116:21116"
    tmpfs:
      - /relais:exec,uid=65534,gid=65534,mode=0700
    read_only: true
    cap_drop: [ALL]
    mem_limit: 256m
```

Pour changer de version, modifier `RELAY_URL` et `RELAY_SHA256`, puis redéployer.

Dans l'application, l'adresse du relais devient alors `wss://relais.exemple.fr`. Un poste en `wss://` et un poste en TCP direct se joignent sans difficulté : seul compte le fait qu'ils parlent au même relais.

- L'en-tête `X-Real-IP` (ou `X-Forwarded-For`) donne au relais l'adresse réelle du client, pour sa limite de tentatives par adresse. Il n'est cru que s'il vient d'un serveur web de la même machine ou d'un réseau privé (cas du relais dans un conteneur).
- Le chiffrement de bout en bout ne change pas : le serveur web ne voit, comme le relais, que des données chiffrées.
- Les versions antérieures à la 0.3.0 ne connaissent que le TCP direct.

## Accès sans surveillance

Pour joindre vos propres machines sans personne devant :

1. Sur chaque machine, définissez un **mot de passe permanent** (8 caractères minimum) et activez **Lancer au démarrage**.
2. Fermer la fenêtre ne quitte pas Distant : il reste dans la zone de notification. Pour l'arrêter, utilisez **Quitter Distant** dans le menu de l'icône.
3. Depuis votre poste, cochez **Mémoriser** à la première connexion : la machine apparaît dans **Mes machines** et se rejoint ensuite en un clic.

Le mot de passe temporaire (affiché dans la fenêtre, renouvelé à chaque lancement) reste valable en parallèle.

## Envoyer des fichiers

Pendant une session, déposez des fichiers dans la fenêtre de contrôle, ou cliquez sur **Envoyer un fichier** dans la barre d'outils. Ils partent un par un, chiffrés comme le reste de la session, et arrivent dans le dossier **Téléchargements** du poste contrôlé.

- Un fichier du même nom n'est jamais remplacé : le nouveau est enregistré sous `nom (1).ext`.
- Un envoi annulé ou interrompu ne laisse rien sur le poste contrôlé : le fichier n'y prend son nom qu'une fois complet.
- La souris et le clavier restent prioritaires sur l'envoi : le poste distant reste utilisable pendant un gros transfert.
- Le poste contrôlé affiche le nom de chaque fichier reçu dans sa fenêtre principale.
- Les dossiers ne sont pas envoyés tels quels : compressez-les d'abord.

## Mises à jour

Une copie installée regarde la dernière version publiée sur GitHub (30 s après son lancement, puis toutes les 6 h), la télécharge, vérifie sa signature et l'installe.

- **Installation automatique** (réglages → *Mises à jour*, activée par défaut sous Windows et Linux) : la nouvelle version est téléchargée en arrière-plan, puis installée dès que le poste n'est plus en session, dans un sens comme dans l'autre. Distant redémarre seul et revient comme il était : réduit dans la zone de notification si sa fenêtre était fermée. Un poste sans surveillance reçoit donc un correctif sans personne devant, et redevient joignable quelques secondes plus tard.
- **À la main** : dès qu'une version est disponible, la fenêtre principale l'annonce avec **Installer et redémarrer**. *Réglages → Rechercher une mise à jour* force une recherche.
- **macOS** : l'installation automatique y est coupée par défaut. L'application n'étant pas signée par un certificat Apple, macOS redemande l'*Enregistrement de l'écran* et l'*Accessibilité* après chaque nouvelle version : un Mac mis à jour sans personne devant ne pourrait plus être contrôlé tant que quelqu'un ne les a pas accordées de nouveau.
- Une mise à jour ne s'applique qu'aux copies installées par un installateur (`.exe`, `.msi`, `.dmg`, `.AppImage`), pas à un binaire lancé depuis `target/`.

## Construire les installateurs

```bash
npm run build
```

Les installateurs sont dans `target/release/bundle/` (`.msi`/`.exe` sous Windows, `.dmg`/`.app` sous macOS, `.deb`/`.AppImage` sous Linux). Tauri ne compile pas d'un système à l'autre : construisez chaque version sur son système, ou publiez une version (ci-dessous).

Le build signe aussi les installateurs pour la mise à jour : il lui faut la clé privée, sinon il échoue.

```bash
TAURI_SIGNING_PRIVATE_KEY="$HOME/.tauri/distant.key" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" npm run build
```

Ces installateurs ne sont pas signés par un certificat. Sous macOS, l'application téléchargée est donc bloquée au premier lancement : après l'avoir copiée dans *Applications*, levez la quarantaine avec `xattr -dr com.apple.quarantine /Applications/Distant.app`. Sous Windows, SmartScreen demande une confirmation (« Informations complémentaires » → « Exécuter quand même »).

## Publier une version

1. Monter la version aux trois endroits : `src-tauri/tauri.conf.json` (celle que comparent les copies installées), `Cargo.toml` (`[workspace.package]`) et `package.json`.
2. Commiter, puis pousser une étiquette du même numéro :

```bash
git tag v0.2.1 && git push origin main v0.2.1
```

3. Le workflow `.github/workflows/release.yml` refuse une étiquette qui diffère de la version, crée une release GitHub en **brouillon**, y envoie les installateurs Windows et macOS (Apple Silicon et Intel) signés, puis écrit `latest.json` à partir de leurs signatures.
4. Essayer les installateurs du brouillon, puis le publier. **Les postes installés ne voient la mise à jour qu'à ce moment-là.**

La clé de signature (minisign, sans mot de passe) est dans `~/.tauri/distant.key` et dans le secret `TAURI_SIGNING_PRIVATE_KEY` du dépôt ; sa moitié publique est dans `tauri.conf.json`. **La perdre empêche à jamais de mettre à jour les copies déjà installées** : gardez-en une sauvegarde.

Pour essayer la recherche de mise à jour sans rien publier, un build de développement accepte un autre `latest.json` (`DISTANT_UPDATE_ENDPOINT=http://127.0.0.1:8000/latest.json`) et un essai à blanc (`DISTANT_UPDATE_DRY_RUN=1`) qui traite la version publiée comme nouvelle, la télécharge et vérifie sa signature, sans l'installer.

## Sécurité

- Le mot de passe ne circule jamais, même chiffré. SPAKE2 empêche un relais malveillant de le deviner hors ligne : chaque essai demande une vraie tentative de connexion.
- Après trois mots de passe erronés, l'hôte refuse les connexions 30 s, puis le double à chaque nouvel échec (jusqu'à 10 min). Une seule connexion entrante est traitée à la fois.
- Le mot de passe permanent et les mots de passe mémorisés ne sont stockés que sous forme dérivée (PBKDF2, 100 000 itérations) dans le fichier de réglages de l'utilisateur. Ce secret dérivé suffit pour se connecter avec Distant : protégez ce fichier comme un mot de passe.
- Quand quelqu'un contrôle le poste, un bandeau l'indique dans la fenêtre principale, avec un bouton pour couper la session.
- Le relais limite les tentatives de connexion à 20 par minute et par adresse IP.

Fichier de réglages : `%APPDATA%\app.distant.desktop\config.json` (Windows), `~/Library/Application Support/app.distant.desktop/config.json` (macOS), `~/.config/app.distant.desktop/config.json` (Linux).

## Autorisations et limites connues

- **macOS** : autorisez Distant dans *Réglages Système → Confidentialité et sécurité* pour l'**Enregistrement de l'écran** (capture) et l'**Accessibilité** (clavier/souris). L'invite apparaît au premier contrôle.
- **Windows** : l'écran de verrouillage, l'invite UAC et `Ctrl+Alt+Suppr` ne sont pas accessibles (il faudrait un service Windows). Une fenêtre lancée en administrateur ne peut pas être contrôlée si Distant ne l'est pas lui-même.
- **Linux** : X11 uniquement pour l'injection clavier/souris. Sous Wayland, la capture passe par le portail du bureau quand il est disponible.
- Pas encore de presse-papiers partagé ni de son. Les fichiers ne vont que du poste qui contrôle vers le poste contrôlé, pas dans l'autre sens.
- Le clavier est transmis en touches physiques : si les deux postes n'ont pas la même disposition (AZERTY/QWERTY), les caractères suivent la disposition du poste contrôlé. `Cmd` et `Ctrl` sont échangés automatiquement entre un Mac et un PC.

## Développement

```bash
cargo test --workspace     # tests unitaires (protocole, chiffrement, tuiles, réglages, fichiers)
cargo clippy --workspace   # lint
RUST_LOG=distant=debug npm run dev
```

Le client de test `probe` se connecte à un hôte sans interface, pour vérifier la réception des images ou envoyer un fichier :

```bash
cargo run -p distant-proto --example probe -- 127.0.0.1:21116 555123456 mot-de-passe
cargo run -p distant-proto --example probe -- 127.0.0.1:21116 555123456 mot-de-passe --send fichier.zip
```

## Licence

MIT
