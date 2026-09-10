# Intention

Ce dépôt est une reprise de voxtype — l'outil de dictée qu'Omarchy installe sous
Linux, et qui marche assez bien pour qu'il n'y ait rien à lui reprocher.

Il n'existe donc pas pour améliorer voxtype. Il existe pour la seule chose que
l'amont ne fera jamais : **Windows**. Le `CLAUDE.md` du projet le déclare
non-objectif — *Linux-first, Wayland-native* — et cette position se défend. Le
MIT règle le reste : ce qui ne sera pas fait là-bas peut l'être ici, sans rien
demander à personne.

Ce qu'on cherche est **un seul outil sur les trois machines** : le même binaire,
le même `config.toml`, le même geste. Pas trois outils de dictée à entretenir, à
régler et à réapprendre, un par poste. Entre Linux et macOS c'est déjà acquis ;
Windows est la pièce manquante, et la seule.

Le dictionnaire de correction est l'endroit où cela se voit le mieux : trois
outils, ce sont trois dictionnaires, les mêmes noms propres à ressaisir et les
mêmes composés français à rattraper, trois fois. Un seul outil, un seul
dictionnaire. C'est la démonstration la plus tangible du motif, pas le motif.

L'horizon dépasse la machine personnelle : des collègues qui dictent en français,
sur des postes d'entreprise, sans qu'un mot quitte la machine. Cela impose trois
choses que rien ne viendra assouplir — un installeur per-user qui passe sans
droits d'administration, un fonctionnement hors-ligne intégral, et le français
tenu pour une exigence et non pour une option.

**Rien n'est décidé.** Ce clone est une option tenue ouverte, pas un chantier
ouvert. Elle se referme ou s'engage sur une mesure, une seule : ce que vaut
réellement Parakeet en français. Si la réponse déçoit, ce dépôt n'a plus de
raison d'être et il monte au grenier.
