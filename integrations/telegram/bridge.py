"""Private JSON-lines bridge. Never print Telegram exceptions or credentials."""
import asyncio
from collections import deque
import json
import logging
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import fcntl
import urllib.parse
import urllib.request

logging.disable(logging.CRITICAL)
MAX_SIZE = 512 * 1024 * 1024
# The bot edits the audio into the posted result: seconds to a minute, sometimes more.
BOT_PREPARE_SECONDS = 180
BOT_POLL_SECONDS = 1.5
# Wait for each audio file before asking the bot to prepare the next one.
PARALLEL = 1
# A busy bot answers a search with nothing, and Telegram keeps that empty answer
# for a while: failed tracks are tried again by themselves, after a pause.
AUTO_RETRIES = 2
RETRY_PAUSE_SECONDS = 45
PLAYLIST_LIMIT = 500
USER_AGENT = 'Mozilla/5.0 (X11; Linux x86_64) Onde'


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


def posted_id(updates):
    """Id of the message a chosen inline result was posted as, when Telegram says."""
    for update in getattr(updates, 'updates', None) or []:
        if type(update).__name__ == 'UpdateMessageID':
            return update.id
        message = getattr(update, 'message', None)
        if type(update).__name__ == 'UpdateNewMessage' and getattr(message, 'id', None):
            return message.id
    return None


def playlist_link(text):
    """('deezer'|'spotify', kind, id) or ('soundcloud', url) for a playlist or album link."""
    text = text.strip()
    match = re.match(r'https?://(?:www\.)?deezer\.com/(?:[a-z]{2}(?:-[a-z]{2})?/)?(playlist|album)/(\d+)', text)
    if match:
        return ('deezer', match[1], match[2])
    match = re.match(r'https?://open\.spotify\.com/(?:intl-[a-z]+/)?(?:embed/)?(playlist|album)/([A-Za-z0-9]+)', text)
    if match:
        return ('spotify', match[1], match[2])
    if re.match(r'https?://(?:www\.|m\.)?soundcloud\.com/[^/?#]+/sets/[^/?#]+', text):
        return ('soundcloud', text.split('?')[0])
    return None


SHORT_LINKS = ('link.deezer.com', 'deezer.page.link', 'spotify.link', 'on.soundcloud.com')


def fetch(url, limit=8 * 1024 * 1024):
    request = urllib.request.Request(url, headers={'User-Agent': USER_AGENT})
    with urllib.request.urlopen(request, timeout=20) as response:
        return response.geturl(), response.read(limit)


def expand_link(text):
    """Share links (link.deezer.com/s/…) redirect to the real playlist page."""
    host = urllib.parse.urlparse(text.strip()).hostname or ''
    if host in SHORT_LINKS:
        final, _ = fetch(text.strip(), 1)
        return final
    return text


# Qualities the bot offers in /settings, and how its buttons name them.
QUALITIES = {'flac': ('flac',), 'mp3_320': ('320',), 'mp3_128': ('128',)}
QUALITY_NAMES = {'flac': 'FLAC', 'mp3_320': 'MP3 320 kb/s', 'mp3_128': 'MP3 128 kb/s'}
SETTINGS_ROUTE = ('qualit', 'quality', 'format', 'audio', 'download', 'telechar')


def settings_choice(choices, target, tried):
    """('pick'|'open', choice) for the next button towards the quality, or None."""
    wanted = QUALITIES[target]
    for choice in choices:
        label = ' '.join(words(choice['label']))
        if any(word in label.split() or word in label for word in wanted):
            return 'pick', choice
    for choice in choices:
        label = ' '.join(words(choice['label']))
        if choice['key'] not in tried and any(part in label for part in SETTINGS_ROUTE):
            return 'open', choice
    return None


def track(artists, title):
    artists = [artist.strip() for artist in artists if artist and artist.strip()]
    return {'artists': artists, 'title': (title or '').strip()[:200]}


def deezer_tracks(kind, ident):
    tracks, index = [], 0
    info = json.loads(fetch(f'https://api.deezer.com/{kind}/{ident}')[1])
    if 'error' in info:
        raise ValueError('introuvable')
    while len(tracks) < PLAYLIST_LIMIT:
        page = json.loads(fetch(f'https://api.deezer.com/{kind}/{ident}/tracks?index={index}&limit=100')[1])
        for item in page.get('data', []):
            tracks.append(track([(item.get('artist') or {}).get('name')], item.get('title')))
        index += 100
        if not page.get('next') or not page.get('data'):
            break
    return info.get('title'), tracks


def spotify_tracks(kind, ident):
    # The embed page carries the track list without an account or API key.
    page = fetch(f'https://open.spotify.com/embed/{kind}/{ident}')[1].decode('utf-8', 'replace')
    match = re.search(r'<script\b[^>]*\bid="__NEXT_DATA__"[^>]*>(.*?)</script>', page, re.S)
    if not match:
        raise ValueError('Spotify ne fournit pas les données de cette playlist')
    data = json.loads(match[1])

    def find_entity(value):
        if isinstance(value, dict):
            if isinstance(value.get('trackList'), list):
                return value
            for child in value.values():
                found = find_entity(child)
                if found is not None:
                    return found
        elif isinstance(value, list):
            for child in value:
                found = find_entity(child)
                if found is not None:
                    return found
        return None

    entity = find_entity(data)
    if entity is None:
        raise ValueError('Spotify ne fournit pas la liste des morceaux')
    # "subtitle" lists every artist: "Fuck Leeches, symbiose dona".
    tracks = []
    for item in entity['trackList']:
        if not isinstance(item, dict):
            continue
        subtitle = item.get('subtitle')
        artists = re.split(r',\s', subtitle) if isinstance(subtitle, str) else []
        title = item.get('title')
        tracks.append(track(artists, title if isinstance(title, str) else ''))
    return entity.get('name') or entity.get('title'), tracks


def soundcloud_track(entry):
    if entry.get('title'):
        return track([entry.get('uploader')], entry['title'])
    # Flat listings only give the page address: "user/track-slug".
    parts = urllib.parse.urlparse(entry.get('url') or '').path.strip('/').split('/')
    if len(parts) < 2:
        return track([], '')
    user, slug = parts[0], parts[1]
    if slug.startswith(user + '-'):
        slug = slug[len(user) + 1:]
    spaced = lambda text: text.replace('-', ' ').replace('_', ' ')
    return track([spaced(user)], spaced(slug))


def soundcloud_tracks(url):
    program = os.environ.get('ONDE_YT_DLP', 'yt-dlp')
    output = subprocess.run([program, '--flat-playlist', '--dump-single-json', '--no-warnings', '--', url],
                            capture_output=True, timeout=120, check=True).stdout
    data = json.loads(output)
    return data.get('title'), [soundcloud_track(entry) for entry in data.get('entries') or []]


def playlist_tracks(text):
    """Playlist name and its tracks, or None when the link is not a playlist."""
    link = playlist_link(expand_link(text))
    if not link:
        return None
    if link[0] == 'deezer':
        name, tracks = deezer_tracks(link[1], link[2])
    elif link[0] == 'spotify':
        name, tracks = spotify_tracks(link[1], link[2])
    else:
        name, tracks = soundcloud_tracks(link[1])
    return name or 'Playlist', [item for item in tracks if item['title']][:PLAYLIST_LIMIT]


def track_label(item):
    return f'{", ".join(item["artists"])} – {item["title"]}' if item['artists'] else item['title']


def clean_title(title):
    """Drops what the bot's catalogue often names differently: featurings, remaster notes."""
    cleaned = re.sub(r'\s*[(\[](?:feat|ft|with|avec)\b[^)\]]*[)\]]', '', title, flags=re.I)
    cleaned = re.sub(r'\s+-\s+[^-]*\b(?:remaster(?:ed)?|version|edit|live|mono|stereo)\b.*$', '',
                     cleaned, flags=re.I)
    return cleaned.strip() or title


def search_queries(item):
    """Searches to try in order, and whether the result must name one of the artists."""
    title = clean_title(item['title'])
    artist = item['artists'][0] if item['artists'] else ''
    candidates = [(f'{artist} {title}', False),
                  (f'{" ".join(words(artist))} {" ".join(words(title))}', False),
                  (title, True), (' '.join(words(title)), True)]
    seen, queries = set(), []
    for query, verify in candidates:
        query = query.strip()
        if query and query.lower() not in seen:
            seen.add(query.lower())
            queries.append((query, verify))
    return queries


def result_words(result):
    text = ' '.join(filter(None, [getattr(result, 'title', None), getattr(result, 'description', None)]))
    for attr in getattr(getattr(result, 'document', None), 'attributes', []) or []:
        text += ' ' + ' '.join(filter(None, [getattr(attr, 'performer', None), getattr(attr, 'title', None)]))
    return set(words(text))


def best_result(results, item, verify):
    """First result naming one of the artists; else the first one, unless it must match."""
    for result in results[:10]:
        text = result_words(result)
        if any(set(words(artist)) and set(words(artist)) <= text for artist in item['artists']):
            return result
    if verify and item['artists']:
        return None
    return results[0] if results else None


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


AUDIO_EXTENSIONS = ('.flac', '.mp3', '.m4a', '.ogg', '.opus', '.wav')


def words(text):
    """Lowercase words without accents, to compare names loosely."""
    import unicodedata
    plain = unicodedata.normalize('NFKD', text).encode('ascii', 'ignore').decode().lower()
    return re.findall(r'[a-z0-9]+', plain)


def library_names(folder):
    """Word sets of the audio files already in the folder, collision suffixes dropped."""
    names = []
    for entry in os.scandir(folder):
        stem, extension = os.path.splitext(entry.name)
        if extension.lower() in AUDIO_EXTENSIONS and not entry.name.startswith('.'):
            names.append(set(words(re.sub(r' \(\d+\)$', '', stem))))
    return names


def already_have(query, names):
    wanted = set(words(query))
    return bool(wanted) and any(wanted <= name for name in names)


def same_track_exists(folder_fd, stem):
    present = set(os.listdir(folder_fd))
    return any(f'{stem}{extension}' in present for extension in AUDIO_EXTENSIONS)


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
    # Download queue: inline results or searches, prepared PARALLEL at a time.
    waiting = deque()
    preparing = {}
    batch = {'name': '', 'total': 0, 'done': 0, 'present': 0, 'failed': [], 'retry': [], 'current': ''}
    runner = None
    later = []
    transfer = asyncio.Lock()
    synthetic = 0
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
        async with transfer:
            return await download_unlocked(message)

    async def download_unlocked(message):
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
            if same_track_exists(descriptor, stem):
                received.add(message.id)
                emit(status=f'Déjà dans la bibliothèque : {stem}')
                return 'present'
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
            return 'new'
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

    def report():
        active = bool(runner and not runner.done() and not batch.get('finished'))
        emit(batch={'name': batch['name'], 'total': batch['total'], 'done': batch['done'],
                    'present': batch['present'],
                    'failed': batch['failed'][-50:], 'current': batch['current'],
                    'active': active})

    def enqueue(items, name):
        nonlocal runner
        if not runner or runner.done():
            batch.update(name=name, total=0, done=0, present=0, failed=[], retry=[], current='',
                         finished=False)
        elif name and batch['name'] != name:
            batch['name'] = 'Plusieurs demandes'
        batch['total'] += len(items)
        waiting.extend(items)
        if not runner or runner.done():
            runner = asyncio.create_task(run_queue())
        report()

    def stop_queue():
        later.clear()
        waiting.clear()
        preparing.clear()
        requested.clear()
        if runner and not runner.done():
            runner.cancel()
        batch['current'] = ''
        batch['finished'] = True
        report()

    def fail(item, reason):
        if item.get('tries', 0) < AUTO_RETRIES:
            item['tries'] = item.get('tries', 0) + 1
            later.append(item)
            return
        batch['failed'].append(f'{item["label"]} ({reason})')
        batch['retry'].append(item)

    async def inline_search(query):
        # The bot sometimes takes too long to answer: try again a little later.
        for attempt in range(3):
            try:
                return await asyncio.wait_for(client(functions.messages.GetInlineBotResultsRequest(
                    bot=bot, peer=bot, query=query, offset='')), 30)
            except Exception as error:
                if attempt == 2 or type(error).__name__ not in ('BotResponseTimeoutError', 'TimeoutError'):
                    raise
                await asyncio.sleep(3 * (attempt + 1))

    async def prepare(item):
        """Ask the bot for one track; returns False when it has nothing for it."""
        nonlocal synthetic
        if 'track' in item:
            found = fallback = None
            for query, verify in search_queries(item['track']):
                results = await inline_search(query)
                chosen = best_result(results.results, item['track'], True)
                if chosen:
                    found = (results.query_id, chosen.id)
                    break
                # Without the artist in the answer, the first result of a full search.
                if fallback is None and not verify and results.results:
                    fallback = (results.query_id, results.results[0].id)
            found = found or fallback
            if not found:
                fail(item, 'introuvable')
                return False
            query_id, result_id = found
        else:
            query_id, result_id = item['inline']
        updates = await client(functions.messages.SendInlineBotResultRequest(
            peer=bot, query_id=query_id, id=result_id))
        ident = posted_id(updates)
        if ident is None:
            synthetic -= 1
            ident = synthetic
        preparing[ident] = (item, time.monotonic())
        return True

    async def run_queue():
        latest = await client.get_messages(bot, limit=1)
        floor = latest[0].id if latest else 0
        while True:
            if not waiting and not preparing:
                if not later:
                    break
                batch['current'] = (f'Nouvel essai automatique pour {len(later)} morceau(x) '
                                    f'dans {RETRY_PAUSE_SECONDS} s…')
                report()
                await asyncio.sleep(RETRY_PAUSE_SECONDS)
                waiting.extend(later)
                later.clear()
                continue
            while waiting and len(preparing) < PARALLEL:
                item = waiting.popleft()
                batch['current'] = item['label']
                report()
                try:
                    await prepare(item)
                except errors.FloodWaitError as error:
                    waiting.appendleft(item)
                    emit(status=f'Limite Telegram : reprise dans {error.seconds} secondes.')
                    await asyncio.sleep(min(error.seconds, 600) + 1)
                except Exception as error:
                    fail(item, type(error).__name__)
                report()
            await asyncio.sleep(BOT_POLL_SECONDS)
            try:
                recent = await client.get_messages(bot, limit=30)
            except Exception:
                recent = []
            for message in reversed(new_audio(recent, bot.id, floor)):
                if message.id in claimed:
                    continue
                # Our own posts carry known ids, unless Telegram did not say which.
                if message.out and message.id not in preparing and not any(key < 0 for key in preparing):
                    continue
                claimed.add(message.id)
                ident = message.id if message.id in preparing else next(
                    (key for key in preparing if key < 0), next(iter(preparing), None))
                entry = preparing.pop(ident, None) if ident is not None else None
                label = entry[0]['label'] if entry else message.id
                batch['current'] = f'Réception : {label}'
                report()
                try:
                    outcome = await asyncio.wait_for(download(message), 600)
                    if entry:
                        batch['done'] += 1
                        batch['present'] += outcome == 'present'
                except Exception as error:
                    if entry:
                        fail(entry[0], type(error).__name__)
            now = time.monotonic()
            for ident, (item, started) in list(preparing.items()):
                if now - started > BOT_PREPARE_SECONDS:
                    del preparing[ident]
                    fail(item, 'le bot n’a rien envoyé en 3 min')
            report()
        batch['current'] = ''
        batch['finished'] = True
        requested.clear()
        report()
        missing = len(batch['failed'])
        present = batch['present']
        emit(status=f'Terminé : {batch["done"]} / {batch["total"]} traités'
             + (f', {present} déjà présents' if present else '')
             + (f', {missing} en échec.' if missing else '.'))

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

    async def press(message, button):
        kind = getattr(button, 'type', button)
        if type(kind).__name__ in ('KeyboardButtonCallback', 'InlineButtonTypeCallback'):
            try:
                await asyncio.wait_for(client(functions.messages.GetBotCallbackAnswerRequest(
                    peer=bot, msg_id=message.id, data=kind.data)), 30)
            except Exception as error:
                # The bot often edits the menu without answering the callback in time.
                if type(error).__name__ not in ('BotResponseTimeoutError', 'TimeoutError'):
                    raise
        else:
            await client.send_message(bot, button.text, parse_mode=None, link_preview=False)

    async def next_menu(floor, seen):
        """Waits for a bot menu newer than floor, or an edit of one already seen."""
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            await asyncio.sleep(BOT_POLL_SECONDS)
            for message in await client.get_messages(bot, limit=5):
                if message.out or message.id <= floor:
                    continue
                version = (message.id, getattr(message, 'edit_date', None), message.raw_text)
                choices = menu_choices(message)
                if choices and version not in seen:
                    seen.add(version)
                    return message, choices
        return None, []

    async def set_quality(target):
        nonlocal request, expires
        name = QUALITY_NAMES[target]
        emit(buttons=[], status=f'Réglage du bot sur {name}… (le bot peut mettre une minute à répondre)')
        sent = await client.send_message(bot, '/settings', parse_mode=None, link_preview=False)
        seen, tried = set(), set()
        for _ in range(5):
            message, choices = await next_menu(sent.id - 1, seen)
            if not message:
                emit(status='Le bot n’a pas répondu à /settings : réessayer plus tard.')
                return
            found = settings_choice(choices, target, tried)
            if not found:
                # Unknown menu: let the user finish by hand.
                menus.clear()
                for choice in choices:
                    menus[choice['key']] = (message, choice['button'])
                request, expires = sent.id, time.monotonic() + 300
                emit(buttons=[{'key': c['key'], 'label': c['label']} for c in choices],
                     status=f'Menu du bot inconnu : choisir {name} ci-dessous.')
                return
            step, choice = found
            tried.add(choice['key'])
            await press(message, choice['button'])
            if step == 'pick':
                emit(quality=target, status=f'Bot réglé sur {name} : les prochains morceaux arrivent dans cette qualité.')
                return
        emit(status=f'{name} introuvable dans les réglages du bot.')

    def queue_results(keys):
        items = []
        for key in keys:
            if key in inline_results and key not in requested:
                requested.add(key)
                query_id, result_id, label = inline_results[key]
                items.append({'inline': (query_id, result_id), 'label': label})
        if items:
            emit(status=f'{len(items)} morceau(x) en file : le bot les prépare un par un, '
                 'réception automatique.' if len(items) > 1 else
                 f'Le bot prépare « {items[0]["label"]} »… Réception automatique dès qu’il est prêt.')
            enqueue(items, items[0]['label'] if len(items) == 1 else 'Sélection')

    async def perform(cmd):
        nonlocal bot, request, expires, folder, client, sending
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
            username = (getattr(bot, 'username', None) or '').lower()
            if not getattr(bot, 'bot', False) or (username and username != 'deezload2bot'):
                raise ValueError('bot')
            menus.clear()
            request = None
            if text.startswith(('https://', 'http://')):
                emit(buttons=[], status='Lecture du lien…')
                try:
                    playlist = await asyncio.wait_for(asyncio.to_thread(playlist_tracks, text), 180)
                except Exception as error:
                    detail = f': {error}' if isinstance(error, AttributeError) else ''
                    emit(status=f'Playlist illisible ({type(error).__name__}{detail}) : lien public requis.')
                    return
                if playlist is not None:
                    name, tracks = playlist
                    names = library_names(folder)
                    unique, seen = [], set()
                    for item in tracks:
                        key = ' '.join(sorted(words(track_label(item))))
                        if key not in seen:
                            seen.add(key)
                            unique.append(item)
                    have = lambda item: already_have(
                        f'{item["artists"][0] if item["artists"] else ""} {clean_title(item["title"])}', names)
                    fresh = [item for item in unique if not have(item)]
                    skipped = len(tracks) - len(fresh)
                    if skipped:
                        emit(status=f'« {name} » : {skipped} morceau(x) déjà présents ou en double, ignorés.')
                    if not fresh:
                        emit(status=f'« {name} » : rien de nouveau à télécharger.')
                        return
                    emit(status=f'« {name} » : {len(fresh)} morceaux à chercher et télécharger. '
                         'Le bot les prépare un par un, ça peut prendre un moment.')
                    enqueue([{'track': item, 'label': track_label(item)} for item in fresh], name)
                    return
            inline_results.clear()
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
                queue_results([key])
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
        elif action == 'queue':
            queue_results(cmd.get('keys') or [])
        elif action == 'quality':
            target = cmd.get('value')
            if target not in QUALITIES:
                return
            if not client or not await client.is_user_authorized():
                raise ValueError('connection')
            bot = bot or await client.get_entity('DeezLoad2Bot')
            if not bot.bot or bot.username.lower() != 'deezload2bot':
                raise ValueError('bot')
            await set_quality(target)
        elif action == 'retry':
            items = batch['retry']
            if not client or not bot or not items or (runner and not runner.done()):
                return
            emit(status=f'Nouvel essai pour {len(items)} morceau(x).')
            for item in items:
                item['tries'] = 0
            name = batch['name'].removeprefix('Réessai · ')
            enqueue(list(items), f'Réessai · {name}')
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
            detail = f': {error}' if isinstance(error, AttributeError) else ''
            emit(status=f'Échec Telegram ({type(error).__name__}{detail}) : vérifier réseau, configuration, session et dossier. Aucun nouvel essai automatique.', qr=[])
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
                stop_queue()
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
                if action in ('receive', 'click', 'send', 'queue', 'retry'):
                    await asyncio.sleep(.1)
                    await commands.put(cmd)
                else:
                    emit(status='Une opération est déjà en cours.')
            else:
                emit(busy=True)
                operation = asyncio.create_task(guarded(cmd))
    finally:
        if runner and not runner.done():
            runner.cancel()
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
