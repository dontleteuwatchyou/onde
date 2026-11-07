import importlib.util
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import types
import unittest

HERE = Path(__file__).parent
spec = importlib.util.spec_from_file_location('bridge', HERE / 'bridge.py')
bridge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bridge)

FAKE = '''
import asyncio
from types import SimpleNamespace as N
class Password(Exception): pass
class Flood(Exception): pass
errors = N(SessionPasswordNeededError=Password, FloodWaitError=Flood)
functions = N(messages=N(GetInlineBotResultsRequest=lambda **kw: ('results', kw),
    SendInlineBotResultRequest=lambda **kw: ('post', kw)))
events = N(NewMessage=lambda **kw: None, MessageEdited=lambda **kw: None)
class TelegramClient:
 def __init__(self, *a, **kw): self.live=False
 async def connect(self): self.live=True
 async def disconnect(self): self.live=False
 def is_connected(self): return self.live
 async def is_user_authorized(self): return True
 def add_event_handler(self, handler, event): self.handler=handler
 async def get_entity(self, name): return N(id=7, bot=True, username=name)
 async def send_message(self, bot, text, **kw):
  if not text.startswith('https://'): raise AssertionError('search must not post')
  async def reply():
   await asyncio.sleep(.05)
   for mid, sender, reply_id in [(50, 8, 42), (51, 7, 42), (52, 7, None)]:
    message=N(id=mid, sender_id=sender, chat_id=sender, out=False,
       reply_to_msg_id=reply_id, audio=True, file=N(size=32), document=N(attributes=[]), raw_text='')
    await self.handler(N(message=message))
  asyncio.create_task(reply())
  return N(id=42)
 async def __call__(self, request):
  if request[0] == 'results':
   return N(query_id=1, results=[N(id='a', title='Titre', description='Artiste'),
       N(id='b', title='Autre', description='Artiste')])
  # Telegram names the posted message; the bot edits the audio into it later.
  self.posts = getattr(self, 'posts', [])
  self.posts.append([60 + len(self.posts), 0])
  update = type('UpdateMessageID', (), {})()
  update.id = self.posts[-1][0]
  return N(updates=[update])
 async def get_messages(self, bot, limit):
  old = N(id=40, out=False, via_bot_id=None, sender_id=7, audio=True)
  posted = []
  for post in getattr(self, 'posts', []):
   post[1] += 1
   posted.append(N(id=post[0], out=True, via_bot_id=7, sender_id=1, audio=post[1] > 2,
       file=N(size=32), document=N(attributes=[]), raw_text=''))
  return posted[::-1] + [old]
 async def download_file(self, document, file, part_size_kb, file_size, progress_callback):
  file.write(b'fLaC' + bytes(28))
  await progress_callback(32,32)
 async def log_out(self): self.live=False; return True
'''

class BridgeTests(unittest.TestCase):
    def test_correlation_rejects_other_chat_and_old_reply(self):
        msg = types.SimpleNamespace(sender_id=7, chat_id=7, out=False, reply_to_msg_id=42)
        self.assertTrue(bridge.correlated(msg, 7, 42))
        self.assertFalse(bridge.correlated(msg, 7, 41))
        msg.chat_id = 8
        self.assertFalse(bridge.correlated(msg, 7, 42))

    def test_menus_exclude_sensitive_and_external_buttons(self):
        def button(kind):
            return types.SimpleNamespace(button=type(kind, (), {'text': kind})(), text=kind)
        message = types.SimpleNamespace(id=9, buttons=[[button('KeyboardButtonCallback'),
            button('KeyboardButtonUrl'), button('KeyboardButtonRequestPhone'), button('KeyboardButton')]])
        choices = bridge.menu_choices(message)
        self.assertEqual([c['key'] for c in choices], ['9:0:0', '9:0:3'])

    def test_raw_markup_works_without_hydrated_buttons(self):
        from telethon.tl.types import ReplyInlineMarkup, KeyboardInlineButtonRow, KeyboardInlineButton, InlineButtonTypeCallback, InlineButtonTypeSwitchInline
        message = types.SimpleNamespace(id=10, buttons=None, reply_markup=ReplyInlineMarkup([
            KeyboardInlineButtonRow([KeyboardInlineButton(text='Artistes', type=InlineButtonTypeCallback(data=b'artist')), KeyboardInlineButton(text='Recherche', type=InlineButtonTypeSwitchInline(query='artist damso'))])]))
        choices = bridge.menu_choices(message)
        self.assertEqual(choices[0]['label'], 'Artistes')
        self.assertEqual(choices[0]['button'].type.data, b'artist')
        self.assertEqual(choices[1]['button'].type.query, 'artist damso')

    def test_result_details_preserve_bot_album_and_artist(self):
        result = types.SimpleNamespace(description='Album : Exemple', document=types.SimpleNamespace(
            attributes=[types.SimpleNamespace(performer='Artiste exemple')]))
        self.assertIn('Album : Exemple', bridge.result_details(result))
        self.assertIn('Artiste exemple', bridge.result_details(result))
        self.assertIn('non précisés', bridge.result_details(types.SimpleNamespace()))

    def test_receives_new_audio_from_the_bot_only(self):
        message = types.SimpleNamespace(id=50, sender_id=7, chat_id=7, out=False, reply_to_msg_id=None)
        self.assertTrue(bridge.should_receive(message, 7, 42))
        self.assertFalse(bridge.should_receive(message, 7, 51))
        message.sender_id = 8
        self.assertFalse(bridge.should_receive(message, 7, 42))

    def test_names_use_tags_and_strip_paths(self):
        import io
        comments = [b'ARTIST=Damso', b'ALBUM=Album', b'TITLE=Monde/../../test']
        data = bytes(4) + len(comments).to_bytes(4, 'little')
        data += b''.join(len(c).to_bytes(4, 'little') + c for c in comments)
        audio = io.BytesIO(b'fLaC' + bytes([132]) + len(data).to_bytes(3, 'big') + data)
        message = types.SimpleNamespace(id=3, file=types.SimpleNamespace(name=None))
        name = bridge.readable_name(audio, message)
        self.assertTrue(name.startswith('Damso - Album - Monde'))
        self.assertNotIn('/', name)

    def test_empty_query_is_explained(self):
        self.assertIn('Saisir', bridge.query_error('   '))
        self.assertIn('4096', bridge.query_error('a' * 4097))
        self.assertIsNone(bridge.query_error('titre'))

    def test_format_rejects_non_audio(self):
        self.assertEqual(bridge.audio_extension(b'fLaC' + bytes(28)), '.flac')
        with self.assertRaises(ValueError):
            bridge.audio_extension(b'<html>not music</html>')

    def test_new_audio_keeps_posts_via_the_bot_and_its_own_audio(self):
        def message(id, out=True, via=7, audio=True, sender=1):
            return types.SimpleNamespace(id=id, out=out, via_bot_id=via, audio=audio, sender_id=sender)
        posted = message(11)
        answer = message(12, out=False, via=None, sender=7)
        messages = [message(9), posted, answer, message(13, via=None),
                    message(14, audio=False), message(15, via=8),
                    message(16, out=False, via=None, sender=8)]
        self.assertEqual(bridge.new_audio(messages, 7, 10), [posted, answer])

    def test_search_posts_nothing_and_click_waits_for_the_edited_audio(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'telethon.py').write_text(FAKE)
            music = root / 'music'
            music.mkdir()
            env = dict(os.environ, PYTHONPATH=str(root), ONDE_TELEGRAM_API_ID='1',
                       ONDE_TELEGRAM_API_HASH='test')
            with subprocess.Popen([sys.executable, '-u', str(HERE / 'bridge.py'), str(root / 'session')],
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                    text=True, env=env) as proc:
                def send(**cmd):
                    proc.stdin.write(json.dumps(cmd) + '\n')
                    proc.stdin.flush()
                def until(predicate):
                    for line in proc.stdout:
                        event = json.loads(line)
                        if predicate(event):
                            return event
                    self.fail('event missing')
                send(action='connect')
                until(lambda e: e.get('connected'))
                until(lambda e: e.get('busy') is False)
                send(action='send', text='une recherche', folder=str(music))
                event = until(lambda e: e.get('buttons'))
                self.assertEqual(event['buttons'][0]['label'], 'Titre')
                send(action='click', key=event['buttons'][0]['key'])
                until(lambda e: 'prépare' in e.get('status', ''))
                self.assertFalse(list(music.iterdir()))
                event = until(lambda e: 'file' in e)
                self.assertTrue(Path(event['file']).exists())
                # The older audio (40) below the floor must stay untouched.
                self.assertEqual(len(list(music.iterdir())), 1)
                send(action='quit')
                self.assertEqual(proc.wait(timeout=5), 0)

    def test_queue_downloads_every_selected_result(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'telethon.py').write_text(FAKE)
            music = root / 'music'
            music.mkdir()
            env = dict(os.environ, PYTHONPATH=str(root), ONDE_TELEGRAM_API_ID='1',
                       ONDE_TELEGRAM_API_HASH='test')
            with subprocess.Popen([sys.executable, '-u', str(HERE / 'bridge.py'), str(root / 'session')],
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                    text=True, env=env) as proc:
                def send(**cmd):
                    proc.stdin.write(json.dumps(cmd) + '\n')
                    proc.stdin.flush()
                def until(predicate):
                    for line in proc.stdout:
                        event = json.loads(line)
                        if predicate(event):
                            return event
                    self.fail('event missing')
                send(action='connect')
                until(lambda e: e.get('busy') is False)
                send(action='send', text='une recherche', folder=str(music))
                keys = [b['key'] for b in until(lambda e: e.get('buttons'))['buttons']]
                send(action='queue', keys=keys + keys)
                event = until(lambda e: e.get('batch', {}).get('total'))
                self.assertEqual(event['batch']['total'], 2)
                event = until(lambda e: 'batch' in e and not e['batch']['active'])
                self.assertEqual(event['batch']['done'], 2)
                self.assertEqual(event['batch']['failed'], [])
                self.assertEqual(len(list(music.iterdir())), 2)
                send(action='quit')
                self.assertEqual(proc.wait(timeout=5), 0)

    def test_tracks_already_in_the_library_are_recognised(self):
        with tempfile.TemporaryDirectory() as tmp:
            for name in ('Vacra - Album - Fille à Papa.flac', 'Damso - Ipséité - Θ. Macarena (1).mp3',
                         'notes.txt', '.onde-telegram-1.part'):
                Path(tmp, name).write_bytes(b'')
            names = bridge.library_names(tmp)
            self.assertEqual(len(names), 2)
            self.assertTrue(bridge.already_have('Vacra Fille a Papa', names))
            self.assertTrue(bridge.already_have('Damso Macarena', names))
            self.assertFalse(bridge.already_have('Vacra Autre titre', names))
            self.assertFalse(bridge.already_have('', names))
            fd = os.open(tmp, os.O_RDONLY | os.O_DIRECTORY)
            try:
                self.assertTrue(bridge.same_track_exists(fd, 'Vacra - Album - Fille à Papa'))
                self.assertFalse(bridge.same_track_exists(fd, 'Vacra - Album - Autre'))
            finally:
                os.close(fd)

    def test_posted_id_reads_the_new_message(self):
        update = type('UpdateMessageID', (), {'id': 77})()
        self.assertEqual(bridge.posted_id(types.SimpleNamespace(updates=[update])), 77)
        message = type('UpdateNewMessage', (), {'message': types.SimpleNamespace(id=78)})()
        self.assertEqual(bridge.posted_id(types.SimpleNamespace(updates=[message])), 78)
        self.assertIsNone(bridge.posted_id(types.SimpleNamespace()))

    def test_playlist_links(self):
        link = bridge.playlist_link
        self.assertEqual(link('https://www.deezer.com/fr/playlist/1109890291?utm=x'),
                         ('deezer', 'playlist', '1109890291'))
        self.assertEqual(link('https://deezer.com/album/12'), ('deezer', 'album', '12'))
        self.assertEqual(link('https://open.spotify.com/intl-fr/playlist/37i9dQZF1DXcBWIGoYBM5M?si=1'),
                         ('spotify', 'playlist', '37i9dQZF1DXcBWIGoYBM5M'))
        self.assertEqual(link('https://soundcloud.com/user/sets/mix?si=2'),
                         ('soundcloud', 'https://soundcloud.com/user/sets/mix'))
        self.assertIsNone(link('https://www.deezer.com/track/3'))
        self.assertIsNone(link('https://test.example/autorise'))

    def test_soundcloud_slug_becomes_a_track(self):
        entry = {'url': 'https://soundcloud.com/sweet-medicine/sweet-medicine-w-odyssee-breezin'}
        self.assertEqual(bridge.soundcloud_track(entry),
                         {'artists': ['sweet medicine'], 'title': 'w odyssee breezin'})
        self.assertEqual(bridge.soundcloud_track({'title': 'Titre', 'uploader': 'Moi'}),
                         {'artists': ['Moi'], 'title': 'Titre'})

    def test_searches_use_the_first_artist_then_the_title(self):
        item = bridge.track(['Fuck Leeches', 'symbiose dona'], 'La dernière vague (feat. X) - Remastered 2020')
        self.assertEqual(bridge.search_queries(item), [
            ('Fuck Leeches La dernière vague', False), ('fuck leeches la derniere vague', False),
            ('La dernière vague', True), ('la derniere vague', True)])
        self.assertEqual(bridge.track_label(item),
                         'Fuck Leeches, symbiose dona – La dernière vague (feat. X) - Remastered 2020')

    def test_best_result_prefers_one_of_the_artists(self):
        result = lambda title, description='': types.SimpleNamespace(id=title, title=title, description=description)
        item = bridge.track(['Fuck Leeches', 'symbiose dona'], 'La dernière vague')
        other, right = result('La dernière vague', 'Quelqu’un'), result('La dernière vague', 'Symbiose Dona')
        self.assertIs(bridge.best_result([other, right], item, True), right)
        self.assertIsNone(bridge.best_result([other], item, True))
        self.assertIs(bridge.best_result([other], item, False), other)

    def test_explicit_lifecycle_and_reception(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'telethon.py').write_text(FAKE)
            music = root / 'music'
            music.mkdir()
            env = dict(os.environ, PYTHONPATH=str(root), ONDE_TELEGRAM_API_ID='1',
                       ONDE_TELEGRAM_API_HASH='test')
            proc = subprocess.Popen([sys.executable, str(HERE / 'bridge.py'), str(root / 'session')],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                text=True, env=env)
            output = queue.Queue()
            def reader():
                for line in proc.stdout:
                    output.put(json.loads(line))
            thread = threading.Thread(target=reader, daemon=True)
            thread.start()
            def send(**cmd):
                proc.stdin.write(json.dumps(cmd) + '\n')
                proc.stdin.flush()
            def until(predicate):
                for _ in range(30):
                    event = output.get(timeout=5)
                    if predicate(event): return event
                self.fail('event missing')
            try:
                self.assertFalse(list(music.iterdir()))
                send(action='connect')
                until(lambda e: e.get('connected'))
                until(lambda e: e.get('busy') is False)
                self.assertFalse(list(music.iterdir()))
                send(action='send', text='https://test.example/autorise', folder=str(music))
                event = until(lambda e: 'file' in e)
                path = Path(event['file'])
                self.assertEqual(path.read_bytes(), b'fLaC' + bytes(28))
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                until(lambda e: 'file' in e)
                self.assertEqual(len(list(music.iterdir())), 2)
                send(action='cancel')
                until(lambda e: e.get('status') == 'Réception arrêtée.')
                send(action='disconnect')
                until(lambda e: e.get('connected') is False)
                session = root / 'session' / 'account.session'
                session.write_text('local-test')
                send(action='delete')
                until(lambda e: e.get('status', '').startswith('Session locale supprimée'))
                self.assertFalse(session.exists())
                send(action='quit')
                self.assertEqual(proc.wait(timeout=5), 0)
                self.assertEqual(proc.stderr.read(), '')
            finally:
                if proc.poll() is None:
                    proc.kill()
                proc.wait()
                proc.stdin.close()
                proc.stdout.close()
                proc.stderr.close()

if __name__ == '__main__':
    unittest.main()
