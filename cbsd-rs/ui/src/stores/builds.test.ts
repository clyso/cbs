/*
 * Copyright © 2026 Clyso GmbH
 *
 *  Licensed under the GNU Affero General Public License, Version 3.0 (the "License");
 *  you may not use this file except in compliance with the License.
 *  You may obtain a copy of the License at
 *
 *  https://www.gnu.org/licenses/agpl-3.0.html
 *
 *  Unless required by applicable law or agreed to in writing, software
 *  distributed under the License is distributed on an "AS IS" BASIS,
 *  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *  See the License for the specific language governing permissions and
 *  limitations under the License.
 */

import { createPinia, setActivePinia } from 'pinia';
import { beforeEach, describe, expect, it } from 'vitest';
import { useBuildsStore } from './builds';
import type { Build } from '@/utils/types/cbs';
import {
  NeutralBuildState,
  SuccessBuildState,
  BuildVersionType,
  BuildArch,
  ErrorBuildState,
} from '@/utils/types/cbs';

function createBuild(overrides: Partial<Build> & { version: string }): Build {
  return {
    task_id: 'task-1',
    user: 'b@b.com',
    submitted: '2024-01-01T00:00:00Z',
    started: '2024-01-01T00:01:00Z',
    finished: '2024-01-01T00:02:00Z',
    state: NeutralBuildState.STARTED,
    desc: {
      version: overrides.version,
      version_type: BuildVersionType.RELEASE,
      channel: 'stable',
      signed_off_by: { email: 'a@b.com', user: 'Alice' },
      dst_image: { name: 'img', tag: 'latest' },
      components: [],
      build: {
        arch: BuildArch.X86_64,
        distro: 'ubuntu',
        os_version: '22.04',
        artifact_type: 'deb',
      },
    },
    ...overrides,
  };
}

const buildA = createBuild({ version: '1.0.0', task_id: 'a' });
const buildB = createBuild({ version: '2.0.0', task_id: 'b' });
const buildC = createBuild({ version: '1.5.0', task_id: 'c' });

describe('buildsStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  describe('computedBuilds — sorting', () => {
    it('returns builds unsorted when no sorter is set', () => {
      const store = useBuildsStore();

      store.builds = [buildB, buildA, buildC];

      expect(store.computedBuilds.map((b) => b.task_id)).toEqual([
        'b',
        'a',
        'c',
      ]);
    });

    it('sorts by version ascending', () => {
      const store = useBuildsStore();

      store.builds = [buildA, buildB, buildC];
      // @ts-expect-error TS2532
      store.sorter = { columnKey: 'version', order: 'ascend' };

      expect(store.computedBuilds.map((b) => b.desc.version)).toEqual([
        '1.0.0',
        '1.5.0',
        '2.0.0',
      ]);
    });

    it('sorts by version descending', () => {
      const store = useBuildsStore();

      store.builds = [buildA, buildB, buildC];
      // @ts-expect-error TS2532
      store.sorter = { columnKey: 'version', order: 'descend' };

      expect(store.computedBuilds.map((b) => b.desc.version)).toEqual([
        '2.0.0',
        '1.5.0',
        '1.0.0',
      ]);
    });

    it('resolves the "version" column key to the nested desc.version path', () => {
      const store = useBuildsStore();

      store.builds = [buildB, buildA]; // 2.0.0, 1.0.0
      // @ts-expect-error TS2532
      store.sorter = { columnKey: 'version', order: 'ascend' };

      // @ts-expect-error TS2532
      expect(store.computedBuilds[0].desc.version).toBe('1.0.0');
    });

    it('sorts by user', () => {
      const store = useBuildsStore();
      const userBuilds = [
        createBuild({ version: '1.0.0', task_id: 'a', user: 'charlie@x.com' }),
        createBuild({ version: '1.0.0', task_id: 'b', user: 'alice@x.com' }),
        createBuild({ version: '1.0.0', task_id: 'c', user: 'bob@x.com' }),
      ];

      store.builds = userBuilds;
      // @ts-expect-error TS2532
      store.sorter = { columnKey: 'user', order: 'ascend' };

      expect(store.computedBuilds.map((b) => b.user)).toEqual([
        'alice@x.com',
        'bob@x.com',
        'charlie@x.com',
      ]);
    });

    it('sorts by state', () => {
      const store = useBuildsStore();
      const stateBuilds = [
        createBuild({
          version: '1.0.0',
          task_id: 'a',
          state: SuccessBuildState.SUCCESS,
        }),
        createBuild({
          version: '1.0.0',
          task_id: 'b',
          state: NeutralBuildState.NEW,
        }),
        createBuild({
          version: '1.0.0',
          task_id: 'a',
          state: ErrorBuildState.FAILURE,
        }),
      ];

      store.builds = stateBuilds;
      // @ts-expect-error TS2532
      store.sorter = { columnKey: 'state', order: 'ascend' };

      // @ts-expect-error TS2532
      expect(store.computedBuilds[0].state).toBe(ErrorBuildState.FAILURE);
      // @ts-expect-error TS2532
      expect(store.computedBuilds[1].state).toBe(NeutralBuildState.NEW);
      // @ts-expect-error TS2532
      expect(store.computedBuilds[2].state).toBe(SuccessBuildState.SUCCESS);
    });
  });

  describe('computedBuilds — pagination', () => {
    it('returns the first page', () => {
      const store = useBuildsStore();

      store.builds = Array.from({ length: 25 }, (_, i) =>
        createBuild({ version: `1.0.${i}`, task_id: `t${i}` }),
      );
      store.page = 1;
      store.pageSize = 10;

      expect(store.computedBuilds).toHaveLength(10);

      store.computedBuilds.forEach((build, index) => {
        expect(build.task_id).toBe(`t${index}`);
      });
    });

    it('returns the second page', () => {
      const store = useBuildsStore();

      store.builds = Array.from({ length: 25 }, (_, i) =>
        createBuild({ version: `1.0.${i}`, task_id: `t${i}` }),
      );
      store.page = 2;
      store.pageSize = 10;

      expect(store.computedBuilds).toHaveLength(10);
      // @ts-expect-error TS2532
      expect(store.computedBuilds[0].task_id).toBe('t10');
    });

    it('returns a partial last page', () => {
      const store = useBuildsStore();

      store.builds = Array.from({ length: 25 }, (_, i) =>
        createBuild({ version: `1.0.${i}`, task_id: `t${i}` }),
      );
      store.page = 3;
      store.pageSize = 10;

      expect(store.computedBuilds).toHaveLength(5);
    });
  });

  describe('pagination computed', () => {
    it('calculates pageCount correctly', () => {
      const store = useBuildsStore();

      store.builds = Array.from({ length: 25 }, (_, i) =>
        createBuild({ version: `1.0.${i}`, task_id: `t${i}` }),
      );
      store.pageSize = 10;

      expect(store.pagination.pageCount).toBe(3);
    });

    it('shows total item count', () => {
      const store = useBuildsStore();

      store.builds = [buildA, buildB, buildC];

      expect(store.pagination.itemCount).toBe(3);
    });
  });
});
