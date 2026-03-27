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
  import {
    CDescriptionItem,
    CDescriptionList,
    CCopyButton,
  } from '@clyso/clyso-ui-kit';
  import { NDivider } from 'naive-ui';
  import type { Build } from '@/utils/types/cbs';
  import { GeneralHelper } from '@/utils/helpers/generalHelper';

  defineProps<{ build: Build }>();
</script>

<template>
  <div class="build-details">
    <section class="build-details__section">
      <p class="build-details__section-title">General</p>
      <CDescriptionList
        :columns="1"
        label-placement="top"
      >
        <CDescriptionItem>
          <template #label>Task ID</template>
          <span
            v-if="build.task_id"
            class="build-details__copy-item"
          >
            <code class="build-details__code">{{ build.task_id }}</code>
            <CCopyButton
              :text="build.task_id"
              size="tiny"
            />
          </span>
          <span v-else>—</span>
        </CDescriptionItem>
      </CDescriptionList>
    </section>

    <NDivider />

    <section class="build-details__section">
      <p class="build-details__section-title">Build</p>
      <CDescriptionList
        :columns="2"
        label-placement="top"
      >
        <CDescriptionItem>
          <template #label>Architecture</template>
          {{ build.desc.build.arch }}
        </CDescriptionItem>
        <CDescriptionItem>
          <template #label>Artifact Type</template>
          {{ build.desc.build.artifact_type }}
        </CDescriptionItem>
        <CDescriptionItem>
          <template #label>Distro</template>
          {{ build.desc.build.distro }} {{ build.desc.build.os_version }}
        </CDescriptionItem>
        <CDescriptionItem>
          <template #label>Channel</template>
          {{ build.desc.channel }}
        </CDescriptionItem>
        <CDescriptionItem :span="2">
          <template #label>Destination Image</template>
          <span class="build-details__copy-item">
            <code class="build-details__code"
              >{{ build.desc.dst_image.name }}:{{
                build.desc.dst_image.tag
              }}</code
            >
            <CCopyButton
              :text="`${build.desc.dst_image.name}:${build.desc.dst_image.tag}`"
              size="tiny"
            />
          </span>
        </CDescriptionItem>
        <CDescriptionItem>
          <template #label>Signed Off By</template>
          {{ build.desc.signed_off_by.user }}
          <span class="build-details__muted">{{
            build.desc.signed_off_by.email
          }}</span>
        </CDescriptionItem>
      </CDescriptionList>
    </section>

    <NDivider />

    <section class="build-details__section">
      <p class="build-details__section-title">Schedule</p>
      <CDescriptionList
        :columns="2"
        label-placement="top"
      >
        <CDescriptionItem>
          <template #label>Submitted</template>
          {{
            build.submitted
              ? GeneralHelper.formatDateTime(build.submitted)
              : '—'
          }}
        </CDescriptionItem>
        <CDescriptionItem>
          <template #label>Started</template>
          {{
            build.started ? GeneralHelper.formatDateTime(build.started) : '—'
          }}
        </CDescriptionItem>
        <CDescriptionItem>
          <template #label>Finished</template>
          {{
            build.started ? GeneralHelper.formatDateTime(build.finished) : '—'
          }}
        </CDescriptionItem>
      </CDescriptionList>
    </section>

    <NDivider />

    <section class="build-details__section">
      <p class="build-details__section-title">Components</p>
      <template v-if="build.desc.components.length">
        <template
          v-for="(component, index) in build.desc.components"
          :key="component.name"
        >
          <NDivider v-if="index > 0" />
          <CDescriptionList
            :columns="2"
            label-placement="top"
          >
            <CDescriptionItem>
              <template #label>Name</template>
              {{ component.name }}
            </CDescriptionItem>
            <CDescriptionItem>
              <template #label>Ref</template>
              <code class="build-details__code">{{ component.ref }}</code>
            </CDescriptionItem>
            <CDescriptionItem v-if="component.repo">
              <template #label>Repo</template>
              <a
                :href="component.repo"
                target="_blank"
                rel="noopener noreferrer"
                class="build-details__link"
                >{{ component.repo }}</a
              >
            </CDescriptionItem>
          </CDescriptionList>
        </template>
      </template>
      <p
        v-else
        class="build-details__muted"
      >
        No components information available
      </p>
    </section>
  </div>
</template>

<style lang="scss" scoped>
  @use '@/styles/utils' as utils;

  .build-details {
    display: flex;
    flex-direction: column;

    &__section {
      padding: utils.unit(3) 0;
    }

    &__section-title {
      margin-bottom: utils.unit(2);
      font-size: 0.75em;
      font-weight: 600;
      letter-spacing: 0.05em;
      text-transform: uppercase;
      color: var(--text-color-3);
    }

    &__link {
      color: var(--primary-color);

      &:hover {
        opacity: 0.8;
      }
    }

    &__code {
      display: inline-block;
      padding: utils.unit(0.5) utils.unit(1);
      font-family: monospace;
      font-size: 0.85em;
      word-break: break-all;
      background: var(--button-color-2);
      border-radius: utils.unit(0.5);
    }

    &__copy-item {
      display: flex;
      align-items: center;
      gap: utils.unit(1);
    }

    &__muted {
      font-size: 0.85em;
      color: var(--text-color-3);
    }
  }
</style>
