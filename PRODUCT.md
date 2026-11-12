# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Application de bureau Tauri 2 (Rust) pour Windows, macOS et Linux. Interface en HTML/CSS/JS vanilla servie depuis `ui/`, sans bundler ni dépendance front, polices système uniquement (l'application doit fonctionner hors ligne). Serveur relais séparé en Rust (`crates/relay`).

## Users

Le propriétaire de plusieurs machines (profil technique) qui veut prendre la main sur ses propres PC et serveurs à distance, la plupart du temps sans personne devant le poste contrôlé. Quand quelqu'un est devant, c'est aussi un utilisateur à l'aise avec l'informatique.

## Product Purpose

Contrôler un ordinateur à distance (écran, clavier, souris) avec un ID à 9 chiffres et un mot de passe, comme TeamViewer ou Supremo, mais en version minimaliste, auto-hébergée et maîtrisée de bout en bout. Réussite : depuis n'importe où, ouvrir une session sur une de ses machines en quelques secondes, avec une image fluide et sans dépendre d'un service tiers.

## Positioning

Relais auto-hébergé et chiffrement de bout en bout (SPAKE2 + ChaCha20-Poly1305) : le relais ne voit ni l'écran, ni les frappes, ni le mot de passe. Code court et lisible, sans compte ni abonnement.

## Operating Context

- Usage typique : depuis un poste principal, ouvrir une session sur une autre machine personnelle (bureau, serveur, PC familial) via le relais.
- Le poste contrôlé tourne sans surveillance : l'application doit démarrer avec la session, rester active dans la zone de notification et accepter un mot de passe permanent.
- Un mot de passe temporaire (régénéré à chaque lancement) reste disponible pour les sessions où quelqu'un est devant le poste.
- Le clavier est transmis en touches physiques (même disposition des deux côtés, typiquement AZERTY).

## Capabilities and Constraints

- Une seule session entrante à la fois ; verrouillage temporaire après trois mots de passe erronés.
- Plusieurs écrans : choix de l'écran partagé pendant la session.
- Qualité d'image réglable (Rapide, Équilibrée, Haute) ; images JPEG par tuiles, accusés de réception pour limiter la latence.
- Envoi de fichiers du poste qui contrôle vers le poste contrôlé (glisser-déposer ou barre d'outils) ; ils arrivent dans son dossier Téléchargements, sans jamais remplacer un fichier existant.
- Non pris en charge pour l'instant : écran de verrouillage et invites UAC de Windows (nécessiterait un service système), presse-papiers, récupération de fichiers depuis le poste contrôlé, envoi de dossiers, son.
- Terminologie : « ID », « mot de passe », « relais », « poste distant », « session ».

## Brand Commitments

- Nom : « Distant ».
- Interface entièrement en français.
- Style : le standard de la catégorie, exécuté avec soin, sans clin d'œil ni décalage. Référence de finition : TeamViewer. Choix explicite de l'utilisateur (octobre 2026) face à des directions plus singulières.

## Evidence on Hand

Aucune donnée d'usage, capture ou témoignage pour l'instant. Ne pas inventer de chiffres de performance, de clients ou de certifications.

## Product Principles

1. Joignable à coup sûr : un poste configuré doit rester accessible sans intervention locale.
2. Rien ne transite en clair : la confidentialité ne dépend jamais du relais.
3. L'essentiel d'abord : ID, mot de passe, connexion ; tout le reste est secondaire.
4. Transparence locale : quand quelqu'un contrôle le poste, cela se voit et se coupe en un clic.
