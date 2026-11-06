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
functions = N()
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
  async def reply():
   await asyncio.sleep(.05)
   for mid, sender, reply_id in [(10, 8, 42), (11, 7, 42), (12, 7, None)]:
    message=N(id=mid, sender_id=sender, chat_id=sender, out=False,
       reply_to_msg_id=reply_id, audio=True, file=N(size=32), document=N(attributes=[]), raw_text='')
    await self.handler(N(message=message))
  asyncio.create_task(reply())
  return N(id=42)
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

    def test_selected_result_receives_new_audio_without_reply(self):
        message = types.SimpleNamespace(id=50, sender_id=7, chat_id=7, out=False, reply_to_msg_id=None)
        self.assertTrue(bridge.should_receive(message, 7, 42, True))
        self.assertFalse(bridge.should_receive(message, 7, 42, False))
        self.assertFalse(bridge.should_receive(message, 7, 51, True))
        message.sender_id = 8
        self.assertFalse(bridge.should_receive(message, 7, 42, True))

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
                send(action='send', text='test autorisé', folder=str(music))
                event = until(lambda e: 'file' in e)
                path = Path(event['file'])
                self.assertEqual(path.read_bytes(), b'fLaC' + bytes(28))
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                self.assertEqual(len(list(music.iterdir())), 1)
                send(action='receive', id=12)
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
