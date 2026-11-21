#!/usr/bin/env python3
"""Complemento de ejemplo para ALarviTerm: un cronómetro por sesión.

No habla con ningún servicio. Guarda en un fichero local cuándo empezó cada
cronómetro y enseña cómo se responde a las órdenes `status` y `action`.
El contrato está en docs/PLUGINS.md.
"""

import json
import os
import sys
import time

RUTA = os.environ.get("CRONOMETRO_FILE") or os.path.join(
    os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state"),
    "alarviterm",
    "cronometro.json",
)


def leer():
    try:
        with open(RUTA, encoding="utf-8") as fichero:
            datos = json.load(fichero)
        return datos if isinstance(datos, dict) else {}
    except (OSError, ValueError):
        return {}


def guardar(datos):
    os.makedirs(os.path.dirname(RUTA), exist_ok=True)
    temporal = f"{RUTA}.{os.getpid()}.tmp"
    with open(temporal, "w", encoding="utf-8") as fichero:
        json.dump(datos, fichero)
    os.replace(temporal, RUTA)


def estado(peticion):
    en_marcha = leer()
    sesiones = {}
    for sesion in peticion.get("sessions", []):
        inicio = en_marcha.get(sesion["id"])
        if inicio:
            sesiones[sesion["id"]] = {
                "timer_since": inicio,
                "tooltip": "Cronómetro en marcha",
                "actions": [
                    {"id": "parar", "label": "Parar el cronómetro",
                     "icon": "media-playback-stop-symbolic", "button": True},
                ],
            }
        else:
            sesiones[sesion["id"]] = {
                "actions": [
                    {"id": "empezar", "label": "Empezar el cronómetro",
                     "icon": "media-playback-start-symbolic", "button": True},
                ],
            }
    return {"sessions": sesiones}


def accion(nombre, peticion):
    sesion = peticion["session"]
    en_marcha = leer()
    if nombre == "empezar":
        en_marcha.setdefault(sesion["id"], int(time.time()))
        guardar(en_marcha)
        return {"ok": True, "message": f"Cronómetro en marcha para {sesion['name']}."}
    if nombre == "parar":
        inicio = en_marcha.pop(sesion["id"], None)
        guardar(en_marcha)
        if inicio is None:
            return {"ok": False, "message": "Ese cronómetro no estaba en marcha."}
        minutos = max(0, int(time.time()) - inicio) // 60
        return {"ok": True, "message": f"{sesion['name']}: {minutos} min."}
    return {"ok": False, "message": f"Acción desconocida: {nombre}."}


def main():
    orden = sys.argv[1] if len(sys.argv) > 1 else ""
    peticion = json.load(sys.stdin)
    if orden == "status":
        respuesta = estado(peticion)
    elif orden == "action" and len(sys.argv) > 2:
        respuesta = accion(sys.argv[2], peticion)
    else:
        sys.exit("uso: cronometro.py status | action <id>")
    json.dump(respuesta, sys.stdout, ensure_ascii=False)


if __name__ == "__main__":
    main()
