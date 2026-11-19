#!/bin/bash
# Download public CC0 sample RAW files from raw.pixls.us for parser tests.
# usage: ./fetch.sh   (idempotent; skips files that already exist)
base="https://raw.pixls.us/data"
dirs=(
"Nikon/Z%208" "Nikon/Z%209" "Nikon/Z%20f" "Nikon/Z%20fc" "Nikon/Z%2050" "Nikon/Z%206" "Nikon/Z%206_2" "Nikon/Z%207" "Nikon/Z%207_2" "Nikon/Z%2030" "Nikon/Z%205"
"NIKON%20CORPORATION/NIKON%20Z6_3" "NIKON%20CORPORATION/NIKON%20Z50_2" "NIKON%20CORPORATION/NIKON%20Z5_2"
"Canon/EOS%20R5" "Canon/EOS%20R6%20Mark%20II" "Canon/EOS%20R50" "Canon/EOS%20R8" "Canon/EOS%205D%20Mark%20IV" "Canon/EOS%206D"
"Sony/ILCE-7M4" "Sony/ILCE-7M3" "Sony/ILCE-6000" "Sony/ILCE-6500" "Sony/ILCE-7CM2"
"Fujifilm/X-T3" "Fujifilm/X-T4" "Fujifilm/X-T5" "Fujifilm/X-H2S" "Fujifilm/X-S10" "Fujifilm/X100V" "FUJIFILM/X-T50" "FUJIFILM/X-M5"
"Panasonic/DC-G9M2" "Panasonic/DC-GH6" "Panasonic/DC-S5" "Panasonic/DC-S5M2" "Panasonic/DC-S1" "Panasonic/DMC-G85" "Panasonic/DC-GH5"
"Leica/C-Lux" "Leica/D-Lux%207" "Leica/D-LUX%20(Typ%20109)"
)
# how many files to take per directory (default 1)
declare -A take=( ["Nikon/Z%208"]=3 ["Nikon/Z%209"]=2 ["Nikon/Z%20f"]=2 ["Nikon/Z%206"]=2 ["Nikon/Z%2050"]=2 ["Nikon/Z%20fc"]=2 ["FUJIFILM/X-M5"]=4 ["Panasonic/DC-G9M2"]=2 ["Panasonic/DC-GH6"]=4 ["Panasonic/DC-S5M2"]=2 )
for d in "${dirs[@]}"; do
  n=${take[$d]:-1}
  out=$(python3 -c "import urllib.parse,sys; print(urllib.parse.unquote(sys.argv[1]).replace('/','__').replace(' ','_'))" "$d")
  files=$(curl -sS -m 40 --retry 4 --retry-all-errors -L "$base/$d/" | grep -o 'href="[^"?/][^"]*"' | sed -E 's/href="|"//g' | grep -iE '\.(nef|cr3|cr2|arw|dng|raf|rw2|rwl)$' | head -n "$n")
  [ -z "$files" ] && { echo "NO FILES: $d"; continue; }
  while IFS= read -r f; do
    name=$(python3 -c "import urllib.parse,sys; print(urllib.parse.unquote(sys.argv[1]).replace(' ','_'))" "$f")
    dest="${out}__${name}"
    if [ -s "$dest" ]; then echo "have $dest"; continue; fi
    echo "get  $dest"
    curl -sS -m 900 --retry 4 --retry-all-errors -L -o "$dest.part" "$base/$d/$f" && mv "$dest.part" "$dest" || echo "FAIL $dest"
  done <<< "$files"
done
echo DONE
