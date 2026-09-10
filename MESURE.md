# Mesure — Parakeet en français

`INTENTION.md` fait dépendre ce dépôt d'une seule réponse : ce que vaut
réellement Parakeet en français. Ce document fixe comment on l'obtient, et
surtout **à quoi on aura reconnu un succès** — écrit avant la mesure, pour que
le résultat ne soit pas négociable après coup.

## La question, formulée exactement

Ce n'est pas « Parakeet est-il bon ». C'est : **Parakeet est-il assez bon en
français, sans GPU, pour qu'on dicte dessus tous les jours.** Whisper large-v3
gagnerait probablement une comparaison de qualité brute, mais il est hors-jeu
sur un poste d'entreprise sans carte. Le concurrent réel de Parakeet 0.6b, c'est
Whisper small ou medium.

## Ce qui existe déjà, et qu'il ne faut pas réécrire

| | |
|---|---|
| Build macOS | `scripts/build-macos.sh VERSION` — déjà `gpu-metal,parakeet-coreml` + tous les moteurs ONNX |
| Modèle par défaut | `parakeet-tdt-0.6b-v3`, le multilingue (`src/config/engines/parakeet.rs:76`) |
| Setup macOS | `src/app/macos.rs:79` télécharge le v3-int8 sans rien demander |
| Comparaison | `voxtype transcribe FICHIER --engine parakeet` — change de moteur sans toucher au `config.toml` |

Prérequis sur le Mac : Xcode Command Line Tools et `cmake`. Le script va
chercher ONNX Runtime 1.24.2 chez Microsoft et le met en cache.

## Le protocole

1. **Build.** `./scripts/build-macos.sh 1.0.1-mesure`
2. **Corpus.** Dix dictées *réelles* — pas un texte lu à voix haute. Ce qu'on
   dicte vraiment : noms propres, composés français, vocabulaire métier, et un
   échantillon en conditions dégradées (bruit de fond, débit rapide).
3. **Passage croisé.** Les mêmes fichiers dans les deux moteurs via `--engine`,
   à `streaming = false` : une variable à la fois.
4. **Dépouillement.** Compter les erreurs *par nature*, pas en taux global.

## Le critère

C'est le point qui décide, et il ne porte pas sur le nombre d'erreurs mais sur
leur nature.

**Ça passe** si les erreurs de Parakeet sont de celles qu'un dictionnaire de
correction rattrape : noms propres, termes récurrents, formes qu'on peut
énumérer une fois pour toutes. Systématiques, donc corrigibles une seule fois.

**Ça ne passe pas** si les erreurs touchent la phrase elle-même : mots avalés,
accords faux, segments inventés, ponctuation qui déplace le sens. Aucun
dictionnaire ne rattrape ça.

La distinction n'est pas arbitraire : tout l'argument de `INTENTION.md` repose
sur un dictionnaire écrit une seule fois. Un moteur dont les erreurs sont
énumérables sert cet argument. Un moteur qui se trompe de phrase le détruit.

## Pièges à ne pas compter comme des échecs

- **int8 contre fp32.** Le v3-int8 est la variante rapide. En cas de doute sur
  la qualité, mesurer d'abord en fp32 ; l'int8 se juge ensuite, sur l'écart.
- **v2 contre v3.** Le v2 est anglais seulement. `docs/PARAKEET.md` le documente
  encore par endroits — l'ignorer, le défaut du code est le bon.
- **Ponctuation absente.** Les modèles TDT ponctuent et capitalisent ; les CTC
  non, et le `CLAUDE.md` prévoit une couche pour ça en 1.2.0. Si on atterrit sur
  un CTC, la ponctuation manquante est une couche absente, pas une mauvaise
  transcription.

## Après

Si ça passe, le build macOS n'est plus un test, c'est la première livraison, et
`ETAT.md` reprend la main sur la couche Windows. Si ça ne passe pas, ce dépôt
monte au grenier — c'est écrit dans `INTENTION.md` et ça n'a pas à être
renégocié ici.
