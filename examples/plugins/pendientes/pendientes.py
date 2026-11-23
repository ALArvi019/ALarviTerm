#!/usr/bin/env python3
"""Complemento de ejemplo: enseña las casillas de TODO.md de la carpeta de cada sesión.

Lee líneas con la forma `- [ ] texto` y `- [x] texto`. En la barra lateral
pone cuántas van hechas, y en la columna de listas, la lista entera.
"""
import json
import os
import re
import sys

CASILLA = re.compile(r"^\s*[-*]\s*\[([ xX])\]\s+(.+?)\s*$")


def leer(carpeta):
    try:
        with open(os.path.join(carpeta, "TODO.md"), encoding="utf-8") as fichero:
            lineas = fichero.readlines()
    except OSError:
        return []
    casillas = (CASILLA.match(linea) for linea in lineas)
    return [{"text": c.group(2), "done": c.group(1) != " "} for c in casillas if c]


def estado(peticion):
    sesiones = {}
    for sesion in peticion.get("sessions", []):
        elementos = leer(sesion.get("cwd", ""))
        if not elementos:
            continue
        hechas = sum(1 for e in elementos if e["done"])
        sesiones[sesion["id"]] = {
            "badge": f"{hechas}/{len(elementos)}",
            "tone": "ok" if hechas == len(elementos) else "muted",
            "tooltip": "Casillas de TODO.md",
            "panel": {"title": "Pendientes", "items": elementos},
        }
    return {"sessions": sesiones}


def main():
    orden = sys.argv[1] if len(sys.argv) > 1 else ""
    peticion = json.load(sys.stdin)
    if orden == "status":
        print(json.dumps(estado(peticion)))
    else:
        print(json.dumps({"ok": False, "message": "Este complemento no tiene acciones"}))


if __name__ == "__main__":
    main()
