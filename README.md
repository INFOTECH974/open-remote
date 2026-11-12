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
- **Presse-papiers partagé** : le texte copié sur un poste se colle sur l'autre, dans les deux sens.
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

Avec **Docker**, chaque version publie l'image `ghcr.io/infotech974/open-remote-relay` (quelques Mo, un seul binaire, utilisateur sans droits). Le port n'est publié qu'en local, c'est le serveur web qui l'ouvre au monde :

```yaml
services:
  relay:
    image: ghcr.io/infotech974/open-remote-relay:0.3.0
    restart: unless-stopped
    ports:
      - "127.0.0.1:21116:21116"
    environment:
      DISTANT_RELAY_KEY: "un-secret-partage"
    read_only: true
    cap_drop: [ALL]
    mem_limit: 256m
```

Dans l'application, l'adresse du relais devient alors `wss://relais.exemple.fr`. Un poste en `wss://` et un poste en TCP direct se joignent sans difficulté : seul compte le fait qu'ils parlent au même relais.

- L'en-tête `X-Real-IP` (ou `X-Forwarded-For`) donne au relais l'adresse réelle du client, pour sa limite de tentatives par adresse. Il n'est cru que s'il vient d'un serveur web de la même machine ou d'un réseau privé (cas du relais dans un conteneur).
- Le chiffrement de bout en bout ne change pas : le serveur web ne voit, comme le relais, que des données chiffrées.
- Les versions antérieures à la 0.3.0 ne connaissent que le TCP direct.

## Accès sans surveillance

Pour joindre vos propres machines sans personne devant :

1. Sur chaque machine, définissez un **mot de passe permanent** (8 caractères minimum) et activez **Lancer au démarrage**.
2. Fermer la fenêtre ne quitte pas Distant : il reste dans la zone de notification. Pour l'arrêter, utilisez **Quitter Distant** dans le menu de l'icône.
3. Depuis votre poste, cochez **Mémoriser** à la première connexion : la machine apparaît dans **Mes machines** et se rejoint ensuite en un clic.

Le mot de passe temporaire (affiché dans la fenêtre, renouvelé à chaque lancement) reste valable en parallèle. Il n'est jamais mémorisé, même avec **Mémoriser** coché : il ne vaudrait plus rien au prochain lancement du poste. Si un mot de passe mémorisé n'est plus accepté (changé sur le poste), Distant l'oublie et propose de le ressaisir.

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
- **À la main** : dès qu'une version est disponible, la fenêtre principale l'annonce avec **Installer et redémarrer**. *Réglages → Rechercher une mise à jour* force une recherche ; si elle trouve une version, les réglages se ferment et une fenêtre propose de l'installer.
- **macOS** : tant que l'application n'est signée que « ad hoc » (sans certificat), macOS la reconnaît à son empreinte, qui change à chaque version, et redemande l'*Enregistrement de l'écran* et l'*Accessibilité* après chaque mise à jour. Un Mac mis à jour à distance ne peut alors plus être contrôlé tant que quelqu'un ne les a pas redonnées sur place. Distant le détecte (`codesign -dv`) : sur un tel Mac il n'installe jamais une mise à jour de lui-même, et prévient avant toute installation demandée à la main.
- Une mise à jour ne s'applique qu'aux copies installées par un installateur (`.exe`, `.msi`, `.dmg`, `.AppImage`), pas à un binaire lancé depuis `target/`.

## Presse-papiers partagé

Pendant une session, le texte copié sur un poste peut être collé sur l'autre, dans les deux sens. Il emprunte le canal chiffré de la session.

- Seul le texte est partagé (jusqu'à 1 Mo), pas les images ni les fichiers : pour un fichier, utilisez l'envoi de fichiers.
- Ce qui était dans le presse-papiers avant la session n'est pas envoyé, seulement ce qui est copié ensuite.
- Côté poste qui contrôle, rien ne part tant que la fenêtre de session n'est pas au premier plan : un texte copié pour une autre application ne voyage que si vous revenez dans la session.
- Le bouton presse-papiers de la barre d'outils suspend le partage dans les deux sens ; le choix est retenu.
- Les deux postes doivent être en version 0.3.3 ou plus récente ; sinon le bouton n'apparaît pas.

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

- **macOS** : Distant doit être autorisé dans *Réglages Système → Confidentialité et sécurité* pour l'**Enregistrement de l'écran** (capture) et l'**Accessibilité** (clavier/souris). Tant qu'il manque une autorisation, la fenêtre principale ouvre un écran *Autorisations de macOS* qui dit laquelle, ouvre le bon réglage et propose de relancer Distant (macOS ne tient compte de l'enregistrement de l'écran qu'au lancement). Le poste qui contrôle est prévenu lui aussi : sans cela il ne verrait qu'un écran vide, sans explication. L'application n'étant pas signée par un certificat Apple, ces autorisations sont à redonner après chaque nouvelle version : les Réglages Système montrent alors Distant toujours coché, mais c'est l'autorisation de l'ancienne version, qui ne vaut plus. Le lien **Réinitialiser les autorisations** de cet écran l'efface (comme `tccutil reset ScreenCapture app.distant.desktop` et `tccutil reset Accessibility app.distant.desktop`), il ne reste qu'à les accorder de nouveau.
- **Windows** : l'écran de verrouillage, l'invite UAC et `Ctrl+Alt+Suppr` ne sont pas accessibles (il faudrait un service Windows). Une fenêtre lancée en administrateur ne peut pas être contrôlée si Distant ne l'est pas lui-même.
- **Linux** : X11 uniquement pour l'injection clavier/souris. Sous Wayland, la capture passe par le portail du bureau quand il est disponible.
- Pas encore de son. Le presse-papiers ne partage que du texte. Les fichiers ne vont que du poste qui contrôle vers le poste contrôlé, pas dans l'autre sens.
- Le clavier est transmis en touches physiques : si les deux postes n'ont pas la même disposition (AZERTY/QWERTY), les caractères suivent la disposition du poste contrôlé. `Cmd` et `Ctrl` sont échangés automatiquement entre un Mac et un PC.

## Développement

```bash
cargo test --workspace     # tests unitaires (protocole, chiffrement, tuiles, réglages, fichiers)
cargo clippy --workspace   # lint
RUST_LOG=distant=debug npm run dev
```

La CI (`.github/workflows/ci.yml`) lance ces tests et le lint sous Windows et sous macOS à chaque push : le code propre à macOS ne se compile que là. Pour voir l'écran des autorisations sans Mac, un build de développement accepte `DISTANT_FAKE_PERMISSIONS` (les autorisations accordées : `input`, `screen,input`, ou vide pour aucune).

Le client de test `probe` se connecte à un hôte sans interface, pour vérifier la réception des images ou envoyer un fichier :

```bash
cargo run -p distant-proto --example probe -- 127.0.0.1:21116 555123456 mot-de-passe
cargo run -p distant-proto --example probe -- 127.0.0.1:21116 555123456 mot-de-passe --send fichier.zip
```

## Licence

MIT
