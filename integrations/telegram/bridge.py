"""Private JSON-lines bridge. Never print Telegram exceptions or credentials."""
import asyncio
import json
import logging
import os
from pathlib import Path
import sys
import time
import fcntl

logging.disable(logging.CRITICAL)
MAX_SIZE = 512 * 1024 * 1024
# The bot edits the audio into the posted result: seconds to a minute, sometimes more.
BOT_PREPARE_SECONDS = 180
BOT_POLL_SECONDS = 1.5


def emit(**event):
    print(json.dumps(event), flush=True)


def menu_choices(message):
    choices = []
    markup = getattr(message, 'reply_markup', None)
    if markup is not None:
        rows = [row.buttons for row in getattr(markup, 'rows', [])]
    else:
        rows = [[button.button for button in row]
                for row in (getattr(message, 'buttons', None) or [])]
    for row_index, row in enumerate(rows):
        for column, button in enumerate(row):
            # Read raw markup: Message.buttons may be None until entities are hydrated.
            kind = getattr(button, 'type', button)
            if type(kind).__name__ in ('InlineButtonTypeCallback', 'InlineButtonTypeSwitchInline', 'ButtonTypeDefault', 'KeyboardButtonCallback', 'KeyboardButton') and not getattr(kind, 'requires_password', False):
                choices.append({'key': f'{message.id}:{row_index}:{column}',
                                'label': button.text[:120], 'row': row_index,
                                'column': column, 'button': button})
    return choices[:100]


def result_details(result):
    # Description is bot-provided; Telegram has no standard album field here.
    details = getattr(result, 'description', None) or ''
    document = getattr(result, 'document', None)
    for attr in getattr(document, 'attributes', []) or []:
        performer = getattr(attr, 'performer', None)
        if performer and performer not in details:
            details = f'Artiste : {performer} · {details}'.strip(' ·')
    return details[:800] or 'Artiste / album non précisés par le bot'


def query_error(text):
    if not text.strip():
        return 'Saisir un titre, un artiste ou un lien dans le champ de recherche avant l’envoi.'
    if len(text.strip()) > 4096:
        return 'Recherche trop longue : maximum 4096 caractères.'
    return None


def correlated(message, bot_id, request_id):
    return (message.sender_id == bot_id and message.chat_id == bot_id
            and not message.out and message.reply_to_msg_id == request_id)


def should_receive(message, bot_id, request_id):
    return (message.sender_id == bot_id and message.chat_id == bot_id
            and not message.out and message.id > request_id)


def new_audio(messages, bot_id, floor_id):
    # A chosen inline result is posted from the user's own account, "via" the bot,
    # which then edits the audio into it. It may also answer with a plain audio message.
    return [m for m in messages if m.audio and m.id > floor_id
            and (m.via_bot_id == bot_id if m.out else m.sender_id == bot_id)]


def readable_name(audio, message):
    tags = {}
    audio.seek(0)
    if audio.read(4) == b'fLaC':
        for _ in range(128):
            header = audio.read(4)
            if len(header) != 4:
                break
            size = int.from_bytes(header[1:], 'big')
            if header[0] & 127 == 4 and size <= 1024 * 1024:
                data = audio.read(size)
                try:
                    pos = 4 + int.from_bytes(data[:4], 'little')
                    count = int.from_bytes(data[pos:pos+4], 'little'); pos += 4
                    for _ in range(min(count, 1000)):
                        length = int.from_bytes(data[pos:pos+4], 'little'); pos += 4
                        entry = data[pos:pos+length].decode('utf-8', errors='replace'); pos += length
                        key, sep, value = entry.partition('=')
                        if sep: tags[key.upper()] = value
                except (ValueError, IndexError):
                    pass
            else:
                audio.seek(size, 1)
            if header[0] & 128:
                break
    artist, title = tags.get('ARTIST'), tags.get('TITLE')
    for attr in getattr(getattr(message, 'document', None), 'attributes', []) or []:
        artist = artist or getattr(attr, 'performer', None)
        title = title or getattr(attr, 'title', None)
    parts = [v for v in (artist, tags.get('ALBUM'), title) if v]
    name = ' - '.join(parts) or getattr(message.file, 'name', None) or f'Audio {message.id}'
    name = ''.join('_' if c in '/\\:*?"<>|' or ord(c) < 32 else c for c in name)
    name = name.strip(' .') or f'Audio {message.id}'
    # Leave room for extension and a collision suffix on common filesystems.
    return name.encode('utf-8')[:180].decode('utf-8', errors='ignore')


def audio_extension(head):
    if head.startswith(b'fLaC'):
        return '.flac'
    if head.startswith(b'OggS'):
        return '.ogg'
    if head.startswith(b'RIFF') and head[8:12] == b'WAVE':
        return '.wav'
    if head.startswith(b'ID3') or (len(head) > 1 and head[0] == 255 and head[1] & 224 == 224):
        return '.mp3'
    if len(head) > 12 and head[4:8] == b'ftyp' and head[8:12] in (b'M4A ', b'M4B '):
        return '.m4a'
    raise ValueError('format')


async def main():
    try:
        from telethon import TelegramClient, events, errors, functions
        import qrcode
    except ImportError:
        emit(status='Dépendances manquantes : voir la section Telegram du README.')
        return
    os.umask(0o077)
    root = Path(sys.argv[1])
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    if root.is_symlink() or root.stat().st_uid != os.getuid():
        emit(status='Répertoire de session non sûr.')
        return
    root.chmod(0o700)
    lock = os.fdopen(os.open(root / 'lock', os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600), 'a')
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        emit(status='Une connexion Onde est déjà ouverte.')
        return
    client = None
    operation = None
    bot = None
    request = None
    expires = 0
    folder = None
    pending = {}
    menus = {}
    inline_results = {}
    received = set()
    early = []
    sending = False
    requested = set()
    claimed = set()
    watcher = None
    watch_floor = 0
    watch_until = 0
    expected = 0
    commands = asyncio.Queue()
    loop = asyncio.get_running_loop()
    # A daemon reader allows EOF/quit to end without a blocked executor thread.
    import threading
    def read():
        try:
            for line in sys.stdin:
                try:
                    value = json.loads(line)
                except ValueError:
                    continue
                loop.call_soon_threadsafe(commands.put_nowait, value)
        finally:
            # stdin closes when Onde exits, even if killed. Never outlive it: a stuck
            # shutdown would keep the session lock and block the next launch.
            watchdog = threading.Timer(5, os._exit, [0])
            watchdog.daemon = True
            watchdog.start()
            loop.call_soon_threadsafe(commands.put_nowait, {'action': 'quit'})
    threading.Thread(target=read, daemon=True).start()

    async def connect():
        nonlocal client
        try:
            api_id = int(os.environ['ONDE_TELEGRAM_API_ID'])
            api_hash = os.environ['ONDE_TELEGRAM_API_HASH']
            if not api_hash:
                raise ValueError()
        except (KeyError, ValueError):
            emit(status=f'Identifiants API absents : renseigner ONDE_TELEGRAM_API_ID et ONDE_TELEGRAM_API_HASH dans {root / "api.env"}.')
            return
        for path in root.glob('account.session*'):
            if path.is_symlink() or path.stat().st_uid != os.getuid():
                raise ValueError('session')
            path.chmod(0o600)
        client = TelegramClient(str(root / 'account'), api_id, api_hash,
                                flood_sleep_threshold=0, request_retries=0)
        await client.connect()
        if not await client.is_user_authorized():
            for _ in range(5):
                qr = await client.qr_login()
                code = qrcode.QRCode(border=4)
                code.add_data(qr.url)
                code.make(fit=True)
                emit(qr=code.get_matrix(), status='Scanner avec Telegram > Paramètres > Appareils.')
                try:
                    await qr.wait(timeout=30)
                    break
                except asyncio.TimeoutError:
                    continue
            else:
                raise TimeoutError()
        client.add_event_handler(incoming, events.NewMessage(incoming=True))
        client.add_event_handler(incoming, events.MessageEdited(incoming=True))
        emit(connected=True, qr=[], status='Connecté. Aucun message envoyé.')

    async def download(message):
        if message.id in received:
            return
        size = message.file.size if message.file else 0
        if not message.audio or not size or size > MAX_SIZE:
            emit(status='Fichier ignoré : audio requis, taille maximale 512 Mio.')
            return
        # Pin the destination directory and use exclusive files, never bot filenames.
        descriptor = os.open(folder, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        temporary = None
        try:
            name = f'.onde-telegram-{message.id}-{os.urandom(8).hex()}.part'
            fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                         0o600, dir_fd=descriptor)
            temporary = name
            last = 0
            started = time.monotonic()
            async def progress(done, total):
                nonlocal last
                if done > MAX_SIZE:
                    raise ValueError('size')
                if time.monotonic() - last > .25:
                    emit(status=f'Réception : {done / 1048576:.1f} / {total / 1048576:.1f} Mio · {done / 1048576 / max(.01, time.monotonic()-started):.2f} Mio/s')
                    last = time.monotonic()
            with os.fdopen(fd, 'wb') as output:
                await client.download_file(message.document, file=output, part_size_kb=512, file_size=size, progress_callback=progress)
                output.flush()
                os.fsync(output.fileno())
            fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=descriptor)
            with os.fdopen(fd, 'rb') as audio:
                if os.fstat(audio.fileno()).st_size != size:
                    raise ValueError('size')
                extension = audio_extension(audio.read(32))
                stem = readable_name(audio, message)
            if stem.lower().endswith(extension):
                stem = stem[:-len(extension)]
            for attempt in range(1000):
                target = f'{stem}{"" if attempt == 0 else f" ({attempt})"}{extension}'
                try:
                    os.link(name, target, src_dir_fd=descriptor, dst_dir_fd=descriptor,
                            follow_symlinks=False)
                    break
                except FileExistsError:
                    continue
            else:
                raise ValueError('collisions')
            received.add(message.id)
            emit(file=str(Path(folder) / target), status=f'Fichier reçu : {target}')
        finally:
            if temporary:
                os.unlink(temporary, dir_fd=descriptor)
            os.close(descriptor)

    def show_results(results):
        inline_results.clear()
        requested.clear()
        choices = []
        for index, result in enumerate(results.results[:50]):
            key = f'inline:{results.query_id}:{index}'
            label = getattr(result, 'title', None) or getattr(result, 'description', None) or f'Résultat {index + 1}'
            inline_results[key] = (results.query_id, result.id, label[:200])
            choices.append({'key': key, 'label': label[:200], 'details': result_details(result)})
        emit(buttons=choices, status='Cliquer sur une musique pour la télécharger.' if choices else 'Aucun résultat.')

    def stop_watching():
        nonlocal expected
        expected = 0
        if watcher:
            watcher.cancel()

    async def watch_for_audio():
        # Polling rather than edit events: history is what Telegram guarantees.
        nonlocal expected, watch_until
        while time.monotonic() < watch_until:
            await asyncio.sleep(BOT_POLL_SECONDS)
            try:
                recent = await client.get_messages(bot, limit=20)
            except Exception:
                continue
            for message in reversed(new_audio(recent, bot.id, watch_floor)):
                if message.id not in claimed:
                    claimed.add(message.id)
                    pending[message.id] = message
                    expected = max(0, expected - 1)
                    await commands.put({'action': 'receive', 'id': message.id})
                    if not expected:
                        # Albums arrive as several files: give the others a moment.
                        watch_until = time.monotonic() + 30
        if expected:
            expected = 0
            emit(status='Le bot n’a pas envoyé le fichier après 3 minutes : cliquer à nouveau pour réessayer.')
            requested.clear()

    async def incoming(event):
        message = event.message
        if sending and bot and message.sender_id == bot.id and len(early) < 100:
            early.append(event)
            return
        if not bot or not request or time.monotonic() > expires:
            return
        if message.sender_id != bot.id or message.chat_id != bot.id or message.out:
            return
        choices = menu_choices(message)
        if choices:
            menus.clear()
            for choice in choices:
                menus[choice['key']] = (message, choice['button'])
            emit(buttons=[{'key': c['key'], 'label': c['label']} for c in choices])
        elif message.id in [entry[0].id for entry in menus.values()]:
            menus.clear()
            emit(buttons=[])
        if message.audio:
            if len(pending) < 100 and message.id not in received:
                pending[message.id] = message
                # Feed downloads through the command loop to keep transfers serialized.
                if should_receive(message, bot.id, request):
                    await commands.put({'action': 'receive', 'id': message.id})
        elif message.raw_text and not message.media:
            # Picture cards would hide the download progress or its failure.
            emit(status='Bot : ' + message.raw_text[:600])

    async def perform(cmd):
        nonlocal bot, request, expires, folder, client, sending
        nonlocal watcher, watch_floor, watch_until, expected
        action = cmd.get('action')
        if action == 'connect':
            if client is None:
                await asyncio.wait_for(connect(), 180)
        elif action == 'send':
            if not client or not await client.is_user_authorized():
                raise ValueError('connection')
            text = cmd.get('text', '').strip()
            problem = query_error(text)
            if problem:
                emit(status=problem)
                return
            try:
                destination = Path(cmd['folder']).resolve(strict=True)
                if not destination.is_dir():
                    raise OSError()
            except (KeyError, OSError):
                emit(status='Dossier musical introuvable : choisir un dossier existant dans Onde.')
                return
            folder = str(destination)
            bot = await client.get_entity('DeezLoad2Bot')
            if not bot.bot or bot.username.lower() != 'deezload2bot':
                raise ValueError('bot')
            inline_results.clear()
            menus.clear()
            request = None
            if not text.startswith(('https://', 'http://')):
                # Answers in under a second and posts nothing, unlike sending the text
                # to the bot, whose menu can take over a minute to come back.
                emit(buttons=[], status='Recherche…')
                show_results(await asyncio.wait_for(client(functions.messages.GetInlineBotResultsRequest(
                    bot=bot, peer=bot, query=text, offset='')), 30))
                return
            emit(buttons=[])
            # NewMessage handler awaits request assignment through this single event loop.
            early.clear()
            sending = True
            try:
                sent = await client.send_message(bot, text, parse_mode=None, link_preview=False)
            finally:
                sending = False
            request = sent.id
            expires = time.monotonic() + 300
            for event in early:
                await incoming(event)
            early.clear()
            emit(status='Lien envoyé au bot : réception automatique dès qu’il répond.')
        elif action == 'click':
            key = cmd.get('key')
            if key in inline_results:
                if key in requested:
                    return
                query_id, result_id, label = inline_results[key]
                latest = await client.get_messages(bot, limit=1)
                await client(functions.messages.SendInlineBotResultRequest(
                    peer=bot, query_id=query_id, id=result_id))
                requested.add(key)
                emit(status=f'Le bot prépare « {label} »… Réception automatique dès qu’il est prêt.')
                expected += 1
                watch_until = time.monotonic() + BOT_PREPARE_SECONDS
                if not watcher or watcher.done():
                    watch_floor = latest[0].id if latest else 0
                    watcher = asyncio.create_task(watch_for_audio())
                return
            choice = menus.get(key)
            if not choice or not request or time.monotonic() >= expires:
                emit(status='Menu expiré : envoyer une nouvelle recherche.', buttons=[])
                return
            message, button = choice
            menus.clear()
            emit(buttons=[])
            expires = time.monotonic() + 300
            kind = getattr(button, 'type', button)
            if type(kind).__name__ == 'InlineButtonTypeSwitchInline':
                show_results(await asyncio.wait_for(client(functions.messages.GetInlineBotResultsRequest(
                    bot=bot, peer=bot, query=kind.query, offset='')), 30))
                return
            if type(kind).__name__ in ('KeyboardButtonCallback', 'InlineButtonTypeCallback'):
                answer = await asyncio.wait_for(client(functions.messages.GetBotCallbackAnswerRequest(
                    peer=bot, msg_id=message.id, data=kind.data)), 30)
            else:
                answer = await client.send_message(bot, button.text, parse_mode=None, link_preview=False)
                request = answer.id
            if getattr(answer, 'message', None):
                emit(status='Bot : ' + answer.message[:600])
        elif action == 'receive':
            message = pending.pop(int(cmd['id']), None)
            if message:
                await asyncio.wait_for(download(message), 600)
        elif action == 'cancel':
            request = None
            pending.clear()
            emit(status='Réception arrêtée.')
        elif action == 'logout':
            request = None
            if not client or not client.is_connected():
                emit(status='Connecter la session avant révocation, ou révoquer dans Telegram > Appareils.')
                return
            await asyncio.wait_for(client.log_out(), 20)
            client = None
            emit(connected=False, status='Session révoquée.', qr=[])

    async def guarded(cmd):
        nonlocal client
        try:
            await perform(cmd)
        except errors.SessionPasswordNeededError:
            if client:
                await client.disconnect()
            client = None
            emit(connected=False, qr=[], status='Telegram exige la 2FA : connexion interrompue, aucun secret demandé.')
        except asyncio.CancelledError:
            if cmd.get('action') == 'connect' and client:
                await client.disconnect()
                client = None
                emit(connected=False)
            emit(status='Opération annulée.', qr=[])
        except errors.FloodWaitError as error:
            emit(status=f'Limite Telegram : attendre {error.seconds} secondes, puis réessayer explicitement.')
        except Exception as error:
            if cmd.get('action') == 'connect' and client:
                await client.disconnect()
                client = None
                emit(connected=False)
            emit(status=f'Échec Telegram ({type(error).__name__}) : vérifier réseau, configuration, session et dossier. Aucun nouvel essai automatique.', qr=[])
        finally:
            emit(busy=False)

    try:
        while True:
            try:
                cmd = await asyncio.wait_for(commands.get(), 1)
            except asyncio.TimeoutError:
                if request and time.monotonic() > expires:
                    request = None
                    menus.clear()
                    emit(buttons=[], status='Fenêtre de réception expirée. Relancer explicitement si nécessaire.')
                continue
            action = cmd.get('action')
            if action in ('quit', 'disconnect', 'delete', 'cancel'):
                if operation and not operation.done():
                    operation.cancel()
                    await operation
                stop_watching()
                request = None
                pending.clear()
                menus.clear()
                emit(buttons=[])
                if action == 'cancel':
                    emit(status='Réception arrêtée.')
                    continue
                if client:
                    await client.disconnect()
                    client = None
                if action == 'delete':
                    for path in root.glob('account.session*'):
                        path.unlink()
                    emit(status='Session locale supprimée. Révoquer aussi dans Telegram > Appareils.')
                else:
                    emit(status='Déconnecté. Session locale conservée.')
                emit(connected=False, qr=[], busy=False)
                if action == 'quit':
                    break
            elif operation and not operation.done():
                # Requeue automatic files rather than lose album replies during transfer.
                if action in ('receive', 'click', 'send'):
                    await asyncio.sleep(.1)
                    await commands.put(cmd)
                else:
                    emit(status='Une opération est déjà en cours.')
            else:
                emit(busy=True)
                operation = asyncio.create_task(guarded(cmd))
    finally:
        stop_watching()
        if operation and not operation.done():
            operation.cancel()
            await operation
        if client:
            await client.disconnect()
        lock.close()


if __name__ == '__main__':
    try:
        asyncio.run(main())
    except Exception:
        emit(status='Service Telegram arrêté : vérifier installation et permissions.')
