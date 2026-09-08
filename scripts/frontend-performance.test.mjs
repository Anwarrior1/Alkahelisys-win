import assert from 'node:assert/strict';
import test from 'node:test';
import { appendUnique, tableWindow } from '../src/pagination.ts';

test('cursor pages preserve order and reject duplicate IDs across and within pages', () => {
  const first = [{ id: '4' }, { id: '3' }];
  const merged = appendUnique(first, [{ id: '3' }, { id: '2' }, { id: '2' }, { id: '1' }]);
  assert.deepEqual(merged.map((item) => item.id), ['4', '3', '2', '1']);
});

test('a page containing no new IDs preserves array identity', () => {
  const first = [{ id: '2' }, { id: '1' }];
  assert.equal(appendUnique(first, [{ id: '1' }, { id: '2' }]), first);
});

test('branching from an earlier immutable page rebuilds a correct ID index', () => {
  const first = [{ id: '4' }, { id: '3' }];
  appendUnique(first, [{ id: '2' }]);
  assert.deepEqual(appendUnique(first, [{ id: '2' }, { id: '1' }]).map((item) => item.id), ['4', '3', '2', '1']);
});

test('table windows bound mounted rows while keeping every loaded row reachable', () => {
  const visited = new Set();
  for (let page = 0; page < 48; page += 1) {
    const window = tableWindow(9_600, page, 200);
    assert.ok(window.end - window.start <= 200);
    for (let index = window.start; index < window.end; index += 1) visited.add(index);
  }
  assert.equal(visited.size, 9_600);
  assert.deepEqual(tableWindow(9_600, 20, 200, true), { page: 20, pages: 48, start: 0, end: 9_600 });
});

test('deletion clamps the visible table page without deriving a cursor', () => {
  assert.deepEqual(tableWindow(199, 2, 200), { page: 0, pages: 1, start: 0, end: 199 });
});
