---
version: 1
slug: "ui-index-html"
primary_target: "ui/index.html"
related_targets: ["ui/viewer.html"]
---

# Surface : fenêtres de l'application (principale et contrôle)

Mode : Operate. Public : propriétaire technique de plusieurs machines, qui les contrôle surtout sans personne devant. Tâche : lire son ID et son mot de passe, régler l'accès sans surveillance une fois, puis rejoindre ses machines en un clic ; dans la fenêtre de contrôle, piloter l'écran distant sans que l'interface gêne.

Contraintes : HTML/CSS/JS vanilla dans `ui/`, polices système, hors ligne, clair et sombre selon le système, fenêtre principale 880×560 (minimum 780×520).

## Direction contract

THESIS: Le standard de la catégorie au niveau de finition de TeamViewer : deux panneaux côte à côte, « Autoriser le contrôle à distance » et « Contrôler un ordinateur distant ». Refuse tout habillage thématique, rétro ou ironique.

OWN-WORLD: Sans-serif système (Segoe UI Variable, SF Pro), chiffres tabulaires pour les ID. Fond neutre clair, panneaux blancs bordés 1 px, rayon 12 px, sans ombre (ombre réservée aux dialogues) ; miroir sombre. Un seul bleu pour l'action principale, la sélection et le focus ; vert, ambre, rouge seulement pour l'état du relais, la session entrante et les refus.

STORY: On lit son ID et son mot de passe d'un coup d'œil, on active une fois l'accès sans surveillance et le démarrage automatique, puis on rejoint « Mes machines » en un clic. Quand quelqu'un contrôle le poste, un bandeau rouge le dit et propose de couper.

FIRST VIEWPORT: Barre haute de 48 px (logo, Distant, nom du poste, réglages). Deux panneaux égaux. Gauche : ID et mot de passe en 30 px semi-gras avec Copier et Nouveau, puis trois interrupteurs. Droite : champs ID et mot de passe, case Mémoriser et bouton bleu « Se connecter », liste défilante des machines. Barre d'état de 36 px : voyant du relais, message, version. Interaction signature : le bouton de connexion porte une barre de progression pendant la mise en relation ; le bandeau de session entrante glisse sous la barre haute et reste.

FORM: Standard de la catégorie (canon), choisi par l'utilisateur à la place du tirage ; référence TeamViewer. Seed key b1a4ccb9.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
