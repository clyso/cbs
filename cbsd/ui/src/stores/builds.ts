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

import { defineStore } from 'pinia';
import { computed, reactive, toRefs } from 'vue';
import type { DataTableSortState } from 'naive-ui';
import type { DataTablePaginationObject } from '@clyso/clyso-ui-kit';
import { CbsService } from '@/services/CbsService';
import type { Build } from '@/utils/types/cbs';
import { GeneralHelper } from '@/utils/helpers/generalHelper';

interface BuildsState {
  isLoading: boolean;
  hasError: boolean;
  builds: Build[];
  sorter: DataTableSortState | null;
  page: number;
  pageSize: number;
  pollingRequest: Promise<unknown> | null;
  pollingTimeout: number | null;
}

const PAGE_SIZES = [10, 20, 30, 50, 100] as const;

function getInitialState(): BuildsState {
  return {
    isLoading: false,
    hasError: false,
    builds: [],
    sorter: null,
    page: 1,
    pageSize: PAGE_SIZES[0],
    pollingRequest: null,
    pollingTimeout: null,
  };
}

export const useBuildsStore = defineStore('buildsStore', () => {
  const state = reactive<BuildsState>(getInitialState());

  const hasNoData = computed<boolean>(() => state.builds.length === 0);

  const columnKeyToPath: Record<string, string> = {
    version: 'desc.version',
    versionType: 'desc.version_type',
  };

  const computedBuilds = computed<Build[]>(() => {
    const builds = state.sorter
      ? GeneralHelper.orderBy(
          state.builds,
          [
            columnKeyToPath[state.sorter.columnKey as string] ??
              state.sorter.columnKey,
          ],
          [state.sorter.order === 'ascend' ? 'asc' : 'desc'],
        )
      : state.builds;

    const start = (state.page - 1) * state.pageSize;
    const end = state.page * state.pageSize;

    return builds.slice(start, end);
  });

  const pagination = computed<DataTablePaginationObject>(() => ({
    page: state.page,
    pageSize: state.pageSize,
    showSizePicker: true,
    pageSizes: [...PAGE_SIZES],
    pageCount: Math.ceil(state.builds.length / state.pageSize),
    itemCount: state.builds.length,
    prefix({ itemCount }) {
      if (state.isLoading || state.hasError) {
        return '';
      }

      return `Total: ${itemCount}`;
    },
  }));

  async function initBuilds() {
    state.isLoading = true;

    try {
      await startBuildsPolling();

      state.hasError = false;
    } catch {
      state.hasError = true;
      await stopBuildsPolling();
    } finally {
      state.isLoading = false;
    }
  }

  async function getBuilds() {
    const res = await CbsService.getBuilds(true);

    state.builds = Array.from(res, ([_, build]) => build);
  }

  async function startBuildsPolling() {
    try {
      await stopBuildsPolling();

      state.pollingRequest = getBuilds();

      await state.pollingRequest;
    } finally {
      state.pollingRequest = null;
      state.pollingTimeout = window.setTimeout(startBuildsPolling, 5000);
    }
  }

  async function stopBuildsPolling() {
    let error: Error | null = null;

    if (state.pollingRequest) {
      try {
        await state.pollingRequest;
      } catch (e) {
        error = e as Error;
      } finally {
        state.pollingRequest = null;
      }
    }

    if (!state.pollingTimeout) {
      return;
    }

    clearTimeout(state.pollingTimeout);
    state.pollingTimeout = null;

    if (error) {
      throw error;
    }
  }

  return {
    ...toRefs(state),
    hasNoData,
    pagination,
    computedBuilds,
    initBuilds,
  };
});
