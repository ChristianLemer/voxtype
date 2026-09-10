# État — 10 septembre 2026

Document local de reprise. Lire `INTENTION.md` pour le pourquoi ; ceci dit où on
en est et ce qui reste à faire.

## Ce que c'est

Fork de [peteonrails/voxtype](https://github.com/peteonrails/voxtype) — Rust, MIT
(© 2025 Peter Jackson), 1432 ⭐, dictée push-to-talk hors-ligne. Dix moteurs
compilés, dont Parakeet et Whisper.

**À ne pas confondre avec VoxScribe** (`Arylmera/vox-scribe`), qui est un projet
sans aucun rapport : C#/Avalonia, Windows, écrit par Guillaume, forké de
`per-simmons/murmur-youtube`. Noms voisins, zéro ligne commune. La confusion a
déjà coûté un détour.

## Câblage git

```
origin    ChristianLemer/voxtype    (le fork, public, créé le 10/09/2026)
upstream  peteonrails/voxtype
```

Branche `dev` — c'est la branche par défaut de l'amont et la base de toute PR,
pas `main`. Elle suit actuellement `upstream/dev` ; si la reprise est assumée,
la basculer sur `origin/dev`.

## Le fait qui commande tout

`CLAUDE.md`, section **Non-Goals**, premier élément :

> - Windows support (Linux-first, Wayland-native)

Ce n'est pas « pas encore ». C'est déclaré, et écrit dans le fichier destiné aux
contributeurs IA. **Toute PR Windows sera refusée sur la politique, pas sur la
qualité.** La feuille de route va jusqu'à 1.5.0 sans une mention de Windows, et
aucune issue utilisateur n'en demande.

Conséquence : ce qui s'écrit ici reste ici. C'est une reprise, pas une
contribution.

## Ce que coûterait la couche Windows

Techniquement plus simple qu'il n'y paraît. `daemon.rs` fait déjà
`use crate::hotkey_macos::{self as hotkey}` : le noyau est écrit contre une
interface plateforme-agnostique, une troisième branche `cfg` s'y insère.

À écrire — environ 600 lignes, sur le modèle exact des fichiers macOS
(`hotkey_macos.rs` 333 l., `output/cgevent.rs` 271 l.) :

1. `src/hotkey_windows.rs` — `rdev` supporte Windows (backend winapi, `listen`
   et `simulate`), et `hotkey_macos.rs` l'utilise déjà. Largement transposable.
2. `src/output/sendinput.rs` — injection via la crate `windows` (MIT/Apache-2.0)
3. un bloc `[target.'cfg(target_os = "windows")'.dependencies]`
4. `.github/workflows/build-windows.yml` — le runner `windows-latest` compile
   nativement, aucune chaîne croisée nécessaire

Déjà acquis sans rien faire : `cpal` a un backend WASAPI, `parakeet-rs` n'est pas
gaté par plateforme et expose DirectML.

Surface de collision mesurée sur 90 jours d'amont (230 commits sur `dev`) :

| Fichier | commits/90j | lignes |
|---|---|---|
| `src/daemon.rs` | **32** | 5024 |
| `Cargo.toml` | 11 | — |
| `src/output/mod.rs` | 4 | 662 |
| `src/lib.rs` | 1 | 106 |
| `src/hotkey/mod.rs` | 0 | — |

Les fichiers neufs ne conflictent jamais. Toute la douleur tient dans
`daemon.rs`, et seulement si l'on adopte des versions amont. Discipline qui rend
la reprise soutenable : **n'ajouter que**, ne jamais réécrire l'existant.

Verrue connue, à traiter si l'on vise trois plateformes propres :
`src/hotkey/mod.rs` définit un trait `HotkeyListener` avec factory, et
`hotkey_macos.rs` ne l'utilise pas — il vit à côté en module séparé. Deux
structures parallèles pour la même chose.

## Licences

Code et dépendances : entièrement permissif, aucune contrainte.

| | |
|---|---|
| voxtype | MIT |
| `parakeet-rs`, `ort`, `tray-icon`, `windows` | MIT OR Apache-2.0 |
| `rdev` | MIT · `cpal`, `evdev` : Apache-2.0 |
| `whisper-rs` | Unlicense (domaine public) |

Le piège est dans les **modèles**, pas le code : les modèles Moonshine
non-anglais sont sous *Community License*, usage non commercial uniquement.
Parakeet appartient à NVIDIA, distinct du code. Sans objet tant qu'on ne
redistribue pas les poids — voxtype les télécharge au `setup`.

Pour mémoire : **VoxScribe et `murmur-youtube` n'ont aucune licence**. Tous
droits réservés. C'est le seul vrai blocage juridique de tout ce dossier, et il
ne concerne pas ce dépôt.

## La question ouverte, qui bloque tout le reste

**Que vaut Parakeet en français ?**

Gratuit à mesurer : voxtype 1.0.1 tourne déjà sur la machine Arch, actif et
configuré en `engine = "parakeet"` avec `parakeet-tdt-0.6b-v3` (le modèle
multilingue). Raccourci F9. Aucune ligne de code Windows ne vaut d'être écrite
avant cette mesure — les utilisateurs visés dictent en français.

Si Parakeet déçoit : neuf autres moteurs sont compilés dans le binaire
(whisper, moonshine, sensevoice, paraformer, dolphin, omnilingual, cohere…)
avant de conclure quoi que ce soit.

## Alternative qui reste sur la table

Ne rien écrire, et prendre trois outils tout faits : voxtype sous Linux (déjà
le cas, via Omarchy), le DMG amont sous macOS, et sous Windows soit VoxScribe
soit [Handy](https://github.com/cjpais/Handy) (MIT, Rust/Tauri, Parakeet V3,
tri-plateforme).

Coût de cette voie : **un `config.toml` dupliqué**, rien de plus — Linux et
macOS partagent déjà le leur puisque c'est le même binaire. C'est la taille
réelle du prix que la couche Windows achèterait.

Recommandation formulée en séance : essayer Handy ou VoxScribe sur la machine
Windows pendant un mois. Si le dictionnaire en double agace, le chantier devient
motivé par l'usage. Sinon, il est évité.

Pour des collègues, Handy a l'avantage du MIT : distribution sans conversation.

## Contraintes non négociables (contexte collègues)

- installeur **per-user, sans droits admin** — le modèle Inno Setup de VoxScribe
- hors-ligne intégral, rien ne quitte la machine
- **français** tenu pour exigence, pas pour option

## Notes de terrain

- Le **cask Homebrew épingle 0.7.5** et son binaire n'embarque pas Parakeet ; la
  formula construit bien avec `--features parakeet` mais épingle 0.6.3. L'amont
  est en 1.0.1. Pour macOS, préférer le DMG des releases ou compiler depuis le
  clone. `voxtype info engines` dit ce que le binaire embarque réellement.
- Build macOS depuis le clone : `cargo build --release --features parakeet`
  (`gpu-metal` pour Metal). Deps : cmake, rust, pkg-config, portaudio.
  **Autorité : `docs/INSTALL_MACOS.md`**, pas la recette brew qui a divergé.
- `INTENTION.md` et ce fichier sont des artefacts locaux : ils apparaîtront dans
  tout diff contre `upstream`. Les commiter (reprise assumée) ou les masquer via
  `.git/info/exclude` — décision non prise.
