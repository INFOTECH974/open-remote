---
name: Distant
description: Prise en main à distance minimaliste, au standard de la catégorie.
colors:
  primary: "#1b5fe0"
  primary-deep: "#164fbd"
  primary-soft: "#e9f0fd"
  neutral-bg: "#f3f4f6"
  surface: "#ffffff"
  surface-hover: "#f5f7fa"
  border: "#e2e5e9"
  border-strong: "#c3c9d2"
  ink: "#14171c"
  ink-muted: "#59626e"
  ink-subtle: "#687180"
  success: "#12a150"
  warning: "#d08a0e"
  danger: "#d42f2f"
  danger-deep: "#b72424"
  danger-ink: "#b42323"
  danger-soft: "#fdeeee"
  stage: "#0f1114"
typography:
  credential:
    fontFamily: "Segoe UI Variable Text, Segoe UI, system-ui, -apple-system, SF Pro Text, Helvetica Neue, Roboto, sans-serif"
    fontSize: "30px"
    fontWeight: 600
    lineHeight: 1.15
    letterSpacing: "0.01em"
    fontFeature: "tnum"
  title:
    fontFamily: "Segoe UI Variable Text, Segoe UI, system-ui, -apple-system, SF Pro Text, Helvetica Neue, Roboto, sans-serif"
    fontSize: "16px"
    fontWeight: 600
    lineHeight: 1.45
    letterSpacing: "-0.01em"
  body:
    fontFamily: "Segoe UI Variable Text, Segoe UI, system-ui, -apple-system, SF Pro Text, Helvetica Neue, Roboto, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.45
  label:
    fontFamily: "Segoe UI Variable Text, Segoe UI, system-ui, -apple-system, SF Pro Text, Helvetica Neue, Roboto, sans-serif"
    fontSize: "13px"
    fontWeight: 500
    lineHeight: 1.4
  meta:
    fontFamily: "Segoe UI Variable Text, Segoe UI, system-ui, -apple-system, SF Pro Text, Helvetica Neue, Roboto, sans-serif"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.4
rounded:
  control: "8px"
  banner: "10px"
  card: "12px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "20px"
  xxl: "24px"
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.surface}"
    typography: "{typography.body}"
    rounded: "{rounded.control}"
    height: "36px"
    padding: "0 16px"
  button-primary-hover:
    backgroundColor: "{colors.primary-deep}"
  button-secondary:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    height: "36px"
    padding: "0 16px"
  button-secondary-hover:
    backgroundColor: "{colors.surface-hover}"
  button-danger:
    backgroundColor: "{colors.danger}"
    textColor: "{colors.surface}"
    rounded: "{rounded.control}"
    height: "30px"
    padding: "0 12px"
  button-danger-hover:
    backgroundColor: "{colors.danger-deep}"
  input:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    height: "36px"
    padding: "0 12px"
  pane:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.card}"
    padding: "20px 22px"
  incoming-banner:
    backgroundColor: "{colors.danger-soft}"
    textColor: "{colors.danger-ink}"
    rounded: "{rounded.banner}"
    padding: "8px 8px 8px 14px"
---

# Design System: Distant

## Overview

**Creative North Star: "Le standard, bien fait"**

Distant reprend volontairement la forme que les utilisateurs connaissent déjà (TeamViewer, AnyDesk) : deux panneaux côte à côte, « Autoriser le contrôle à distance » et « Contrôler un ordinateur distant », une barre haute et une barre d'état. Ce choix est un engagement de l'utilisateur, pas une facilité : la réussite se mesure à la finition, pas à l'originalité.

L'interface est calme et dense sans être serrée. Les informations à lire d'un coup d'œil (ID, mot de passe) sont grandes ; tout le reste est en 12 à 14 px. La couleur ne sert qu'à agir (bleu) ou à signaler un état (vert, ambre, rouge). Le thème suit le système, clair ou sombre, avec les mêmes rôles.

Rejets confirmés : habillage rétro ou thématique (Minitel, cartes QSL, console « hacker »), clin d'œil ou décalage.

**Key Characteristics:**
- Polices système, chiffres tabulaires pour les ID, mots de passe et mesures.
- Panneaux blancs bordés d'un trait, sans ombre ; l'ombre est réservée à ce qui flotte (dialogues, superposition de session).
- Un seul bleu d'action ; couleurs d'état uniquement là où l'état compte.
- Icônes SVG dessinées au trait de 1,5 px sur une grille 16×16.
- La fenêtre de contrôle s'efface : barre d'outils de 48 px, écran distant sur fond presque noir.

## Colors

Une palette neutre légèrement froide, un bleu franc pour agir, trois couleurs d'état. Chaque jeton a son équivalent sombre (voir `ui/style.css`, bloc `prefers-color-scheme: dark`).

### Primary
- **Bleu Distant** (#1b5fe0, sombre #2a64d8) : bouton principal (« Se connecter », « Connexion » sur une machine mémorisée), interrupteurs actifs, logo. Texte blanc dessus ≥ 4,5:1.
- **Bleu profond** (#164fbd, sombre #2459c4) : survol du bouton principal.
- **Voile bleu** (#e9f0fd, sombre #1b2944) : halo de focus des champs, boutons de barre d'outils enfoncés, sélection de texte.
- **Bleu texte** (#1b5fe0, sombre #7aa7ff) : liens, anneaux de focus, curseur de saisie.

### Neutral
- **Gris de fond** (#f3f4f6, sombre #131519) : fond de fenêtre derrière les panneaux.
- **Surface** (#ffffff, sombre #1c1f25) : panneaux, barres haute et d'état, dialogues, champs.
- **Surface survolée** (#f5f7fa, sombre #242830) : survol des lignes de machines, boutons icône, boutons secondaires.
- **Trait** (#e2e5e9, sombre #2c3139) : bordure des panneaux et séparateurs.
- **Trait marqué** (#c3c9d2, sombre #414854) : bordure des champs et boutons secondaires, piste d'interrupteur inactif.
- **Encre** (#14171c, sombre #e7e9ec) : texte principal.
- **Encre atténuée** (#59626e, sombre #a3abb6) : libellés, descriptions, métadonnées.
- **Encre discrète** (#687180, sombre #939ca8) : dates, adresse du relais, version, icônes de lignes.
- **Scène** (#0f1114) : fond de la fenêtre de contrôle autour de l'écran distant, identique dans les deux thèmes.

### État
- **Vert en ligne** (#12a150, sombre #3fbf75) : voyant du relais connecté, coche « copié ».
- **Ambre** (#d08a0e, sombre #e3a530) : voyant du relais en cours de connexion.
- **Rouge alerte** (#d42f2f, sombre #c93434) : voyant d'erreur, bouton « Couper la session » et « Terminer ». Encre rouge (#b42323, sombre #ff8c8c) sur voile rouge (#fdeeee, sombre #3a1f22) pour le bandeau de session entrante et les refus.

### Named Rules
**The One Blue Rule.** Le bleu ne sert qu'à l'action principale, au focus et à la sélection. Une seule action bleue par zone.

**The Earned Red Rule.** Le rouge signifie « quelqu'un contrôle ce poste » ou « tentative refusée ». Jamais pour décorer, jamais pour une action courante.

## Typography

**Police unique :** Segoe UI Variable / Segoe UI (Windows), SF Pro (macOS), puis la sans-serif système.

**Character :** une seule famille, celle du système, pour que l'application paraisse native partout. La hiérarchie passe par la taille et la graisse.

### Hierarchy
- **Credential** (600, 30px, 1,15, +0,01em, chiffres tabulaires) : ID et mot de passe de ce poste. Rien d'autre n'a cette taille.
- **Title** (600, 16px, -0,01em) : titres des panneaux ; 17px pour les dialogues et la superposition de session.
- **Body** (400, 14px, 1,45) : texte courant, champs (15px), boutons (600, 14px).
- **Label** (500, 13px) : libellés de champs, descriptions de panneaux, interrupteurs, barre d'outils.
- **Meta** (400, 12px) : ID et système des machines, dates, statistiques de session, barre d'état (12,5px).

### Named Rules
**The Tabular Figures Rule.** Tout ce qui se compare ou se dicte (ID, mots de passe, heures, débits) est en chiffres tabulaires, groupés par trois ou quatre.

## Layout

Fenêtre principale de 880×560 (minimum 780×520) en colonne : barre haute de 48 px, bandeau de session entrante facultatif, deux panneaux égaux (grille 1fr 1fr, écart et marge de 16 px), bande de notification facultative, barre d'état de 34 px. Les bandeaux masqués ne réservent aucune place. Dans les panneaux, rythme de 20 px entre blocs, 12 à 14 px à l'intérieur d'un bloc. La liste « Mes machines » prend la hauteur restante et défile.

Sous 840 px de large, la date de dernière connexion disparaît des lignes de machines. La fenêtre de contrôle (1280×800 par défaut, minimum 640×400) retire progressivement les libellés de sa barre d'outils (1100, 960, 720 px) sans jamais retirer de contrôle. En plein écran, la barre se range et revient au survol du bord haut.

## Elevation & Depth

Système plat. La profondeur vient du contraste entre le gris de fond et la surface blanche, et d'un trait d'un pixel. L'ombre n'existe que pour ce qui flotte au-dessus du contenu.

### Shadow Vocabulary
- **Flottant** (`box-shadow: 0 18px 48px rgb(15 20 30 / 0.18), 0 2px 8px rgb(15 20 30 / 0.08)` ; sombre `0 18px 48px rgb(0 0 0 / 0.5), 0 2px 8px rgb(0 0 0 / 0.3)`) : dialogues et superposition de connexion/fin de session.
- **Barre en plein écran** (`box-shadow: 0 6px 24px rgb(0 0 0 / 0.35)`) : barre d'outils dévoilée en plein écran.

### Named Rules
**The Border-Or-Shadow Rule.** Un élément porte un trait ou une ombre, jamais les deux.

## Shapes

Coins doucement arrondis, deux rayons : 8px pour les contrôles (boutons, champs, lignes de liste), 12px pour les conteneurs (panneaux, dialogues, superpositions), 10px pour le bandeau de session entrante. Interrupteurs en pilule (34×20). Icônes au trait arrondi.

## Components

### Buttons
- **Shape :** coins de 8px, hauteur 36px (28 à 30px en version compacte).
- **Primary :** fond Bleu Distant, texte blanc 600 14px, marge interne 0 16px ; survol en Bleu profond.
- **Secondary :** fond surface, trait marqué, texte encre ; survol en surface survolée.
- **Danger :** fond rouge alerte, texte blanc, réservé à couper une session.
- **Icon :** 32×32, transparent, icône en encre atténuée, fond surface survolée au survol ; l'icône « copier » devient une coche verte pendant 1,4 s.
- **Progression :** pendant une connexion, le bouton principal affiche « Connexion… » et une barre blanche de 3px qui avance une seule fois de 0 à 90 % (12 s, sortie exponentielle).

### Inputs / Fields
- **Style :** surface, trait marqué, coins 8px, hauteur 36px, texte 15px.
- **Focus :** trait en bleu texte et halo de 3px en voile bleu.
- **Erreur :** message en encre rouge sous le formulaire, jamais de champ rougi seul.

### Switches
- Piste 34×20 en trait marqué, Bleu Distant une fois activée ; pastille blanche de 14px qui glisse en 160 ms.

### Cards / Containers
- **Corner Style :** 12px.
- **Background :** surface.
- **Shadow Strategy :** aucune (voir Elevation).
- **Border :** 1px trait.
- **Internal Padding :** 20px 22px.

### Mes machines (ligne de liste)
Icône d'écran, nom en 600 13,5px, ID et système en meta, date, clé si le mot de passe est mémorisé, bouton « Connexion » (principal) ou « Choisir » (secondaire), bouton « Retirer » visible au survol. Fond surface survolée au survol, coins 8px.

### Barre d'état
Voyant de 8px (vert, ambre, rouge, gris), message en encre, adresse du relais et version en encre discrète.

### Envoi de fichiers (fenêtre de contrôle)
Liste flottante en bas à droite de l'écran distant (surface, coins 12px, ombre Flottant, 340px), visible seulement tant qu'elle contient un envoi. Par ligne : icône de fichier, nom en 600 13,5px, état en meta tabulaire (« 1,2 Mo sur 120 Mo · 1 % », « Reçu dans Téléchargements »), barre de progression bleue de 3px, bouton icône pour annuler ou retirer. Une réussite affiche une coche verte et s'efface seule après 8 s ; une erreur reste en encre rouge jusqu'à ce qu'on la retire. Pendant le survol de fichiers, un voile sombre couvre l'écran distant avec un encart « Déposez pour envoyer ».

Côté poste contrôlé, chaque fichier reçu s'annonce dans la bande de notification en version neutre (surface, trait, encre) : le rouge reste réservé aux tentatives refusées.

### Bandeau de session entrante
Voile rouge, trait rouge clair, encre rouge 600 ; glisse de 8px vers le bas en 280 ms et reste tant que la session dure, avec « Couper la session » à droite.

## Do's and Don'ts

### Do:
- **Do** garder la structure deux panneaux de la catégorie, avec l'ID et le mot de passe en Credential (30px, 600, tabulaire).
- **Do** suivre le thème du système et définir chaque nouvelle couleur en clair et en sombre.
- **Do** dessiner toute nouvelle icône en SVG 16×16, trait de 1,5px, extrémités arrondies.
- **Do** viser au moins 4,5:1 de contraste pour le texte, survols compris.
- **Do** respecter `prefers-reduced-motion` : les animations durent alors 1 ms.

### Don't:
- **Don't** introduire d'habillage rétro, thématique ou ironique : l'utilisateur a choisi le standard de la catégorie.
- **Don't** utiliser de caractère Unicode ou d'emoji à la place d'une icône.
- **Don't** mettre un trait et une ombre sur le même élément, ni d'ombre sur les panneaux.
- **Don't** employer le rouge pour autre chose qu'une session en cours ou un refus.
- **Don't** faire boucler une animation en continu ; une progression avance et s'arrête.
