<!--
  - Copyright © 2026 Clyso GmbH
  -
  -  Licensed under the GNU Affero General Public License, Version 3.0 (the "License");
  -  you may not use this file except in compliance with the License.
  -  You may obtain a copy of the License at
  -
  -  https://www.gnu.org/licenses/agpl-3.0.html
  -
  -  Unless required by applicable law or agreed to in writing, software
  -  distributed under the License is distributed on an "AS IS" BASIS,
  -  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
  -  See the License for the specific language governing permissions and
  -  limitations under the License.
  -->

<script setup lang="ts">
  import type {
    DataTableBaseColumn,
    DataTableSelectionColumn,
    DataTableSortState,
  } from '@clyso/clyso-ui-kit';
  import { CDataTable, CTooltip } from '@clyso/clyso-ui-kit';
  import { storeToRefs } from 'pinia';
  import { computed, ref } from 'vue';
  import { NDrawer, NDrawerContent } from 'naive-ui';
  import { formatDistanceToNow } from 'date-fns';
  import BuildStateCell from './BuildStateCell.vue';
  import BuildDetails from './BuildDetails.vue';
  import type { Build } from '@/utils/types/cbs';
  import { useBuildsStore } from '@/stores/builds';
  import { GeneralHelper } from '@/utils/helpers/generalHelper';

  const {
    isLoading,
    computedBuilds,
    hasError,
    sorter,
    page,
    pageSize,
    pagination,
  } = storeToRefs(useBuildsStore());

  const { initBuilds } = useBuildsStore();

  const selectedBuild = ref<Build | null>(null);

  const rowProps = (row: Build) => ({
    onClick: () => {
      selectedBuild.value = row;
    },
  });

  const columns = computed<
    (DataTableBaseColumn<Build> | DataTableSelectionColumn)[]
  >(() => [
    {
      title: 'State',
      key: 'state',
      sorter: true,
    },
    {
      title: 'User',
      key: 'user',
      sorter: true,
    },
    {
      title: 'Version',
      key: 'version',
      sorter: true,
    },
    {
      title: 'Finished',
      key: 'finished',
      sorter: true,
    },
  ]);

  const handleSortingChange = (newSorter: DataTableSortState | null) => {
    if (!computedBuilds.value.length) {
      return;
    }

    sorter.value = newSorter;
    page.value = 1;
  };

  const handlePageChange = (newPage: number) => {
    page.value = newPage;
  };

  const handlePageSizeUpdate = (newPageSize: number) => {
    pageSize.value = newPageSize;
    page.value = 1;
  };

  function formatDatetime(dateTime: string) {
    return GeneralHelper.formatDateTime(dateTime);
  }
</script>

<template>
  <div class="builds-list">
    <div class="builds-list__container">
      <CDataTable
        class="builds-list__table"
        :data="computedBuilds"
        :columns="columns"
        max-height="1020px"
        :is-controlled="true"
        :is-loading="isLoading"
        :has-error="hasError"
        :pagination="pagination"
        :bordered="false"
        :sorter="sorter"
        :row-props="rowProps"
        @update:sorter="handleSortingChange"
        @update:page="handlePageChange"
        @update:page-size="handlePageSizeUpdate"
        @retry="initBuilds"
      >
        <template #user="{ rowData }">
          {{ rowData.user }}
        </template>

        <template #version="{ rowData }">
          <span>{{ rowData.desc.version }}</span>
          <span class="builds-list__version-type">{{
            rowData.desc.version_type
          }}</span>
        </template>

        <template #state="{ rowData }">
          <BuildStateCell :build="rowData" />
        </template>

        <template #finished="{ rowData }">
          <CTooltip
            v-if="rowData.finished"
            placement="top"
          >
            <template #trigger>
              {{
                formatDistanceToNow(new Date(rowData.finished), {
                  addSuffix: true,
                })
              }}
            </template>
            {{ formatDatetime(rowData.finished) }}
          </CTooltip>
        </template>
      </CDataTable>
    </div>

    <NDrawer
      :show="!!selectedBuild"
      :width="550"
      placement="right"
      @update:show="selectedBuild = null"
    >
      <NDrawerContent closable>
        <template #header>
          <span class="builds-list__drawer-header">
            <BuildStateCell
              v-if="selectedBuild"
              :build="selectedBuild"
            />
            <span class="builds-list__drawer-title">{{
              selectedBuild?.desc.version
            }}</span>
          </span>
        </template>
        <BuildDetails
          v-if="selectedBuild"
          :build="selectedBuild"
        />
      </NDrawerContent>
    </NDrawer>
  </div>
</template>

<style lang="scss" scoped>
  @use '@/styles/utils' as utils;

  .builds-list {
    &__drawer-header {
      display: flex;
      align-items: center;
      gap: utils.unit(2);
      min-width: 0;
    }

    &__drawer-title {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }

    :deep(.c-data-table-tr) {
      cursor: pointer;
    }

    &__version-type {
      margin-left: utils.unit(1);
      font-size: 0.8em;
      opacity: 0.5;
    }
  }
</style>
